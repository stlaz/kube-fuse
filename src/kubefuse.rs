use std::{
    collections::HashMap,
    sync::atomic::AtomicU64,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fuser::{self, FileAttr};
use libc;

use k8s_openapi::api::core::v1::Namespace;

use client_rs::{corev1::CoreV1Client, rest};

const BLOCK_SIZE: u32 = 512;

const ROOT_ATTR: FileAttr = FileAttr {
    ino: 1,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: fuser::FileType::Directory,
    perm: 0o755,
    nlink: 2,
    uid: 1000,
    gid: 1000,
    rdev: 0,
    flags: 0,
    blksize: BLOCK_SIZE,
};

const TTL: Duration = Duration::from_secs(1);

type InodeTable = HashMap<u64, Node>;
struct Node {
    name: String,
    attrs: FileAttr,
    content: NodeContent,
}

impl Node {
    fn children_mut(&mut self) -> Option<&mut NodeChildren> {
        match &mut self.content {
            NodeContent::Children(children) => Some(children),
            NodeContent::Bytes(_) => None,
        }
    }
}

type NodeChildren = HashMap<String, u64>;
enum NodeContent {
    Bytes(Vec<u8>),
    Children(NodeChildren),
}
pub struct KubeFilesystem<'c> {
    // Add fields as necessary
    core_client: CoreV1Client<'c>,

    inodes: InodeTable,
    inode_counter: AtomicU64,
}

impl<'c> KubeFilesystem<'c> {
    pub fn new(rest_client: &'c rest::RestClient) -> Self {
        KubeFilesystem {
            core_client: CoreV1Client::new(rest_client),

            inodes: InodeTable::new(),
            inode_counter: AtomicU64::new(2),
        }
    }
}

impl<'c> KubeFilesystem<'c> {
    fn next_inode(&self) -> u64 {
        self.inode_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    fn create_namespace_node(&mut self, inode: u64, namespace: &Namespace) {
        let creation_time = namespace
            .metadata
            .creation_timestamp
            .as_ref()
            .and_then(|t| t.0.timestamp().try_into().ok())
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap_or(UNIX_EPOCH);

        let mut children = NodeChildren::new();
        let manifest_ino = self.next_inode();
        children.insert("manifest.yaml".to_string(), manifest_ino);

        let ns_node = Node {
            name: namespace.metadata.name.clone().unwrap_or_default(),
            attrs: FileAttr {
                ino: inode,
                size: 0,
                blocks: 0,
                atime: creation_time,
                mtime: creation_time,
                ctime: creation_time,
                crtime: creation_time,
                kind: fuser::FileType::Directory,
                perm: 0o755,
                nlink: 2, // FIXME: should be updated when we add children directories
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: BLOCK_SIZE,
            },
            content: NodeContent::Children(children),
        };
        self.inodes.insert(ns_node.attrs.ino, ns_node);

        let ns_yaml = serde_yaml::to_string(namespace)
            .unwrap_or_default()
            .into_bytes();
        let ns_yaml_size = ns_yaml.len() as u64;
        let manifest_node = Node {
            name: "manifest.yaml".to_string(),
            attrs: FileAttr {
                ino: manifest_ino,
                size: ns_yaml_size,
                blocks: ns_yaml_size.div_ceil(u64::from(BLOCK_SIZE)),
                atime: creation_time,
                mtime: creation_time,
                ctime: creation_time,
                crtime: creation_time,
                kind: fuser::FileType::RegularFile,
                perm: 0o444,
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: BLOCK_SIZE,
            },
            content: NodeContent::Bytes(ns_yaml),
        };
        self.inodes.insert(manifest_ino, manifest_node);
    }

    fn namespace_inode(&self, namespace: &str) -> Option<u64> {
        self.inodes.get(&1).and_then(|root| match &root.content {
            NodeContent::Children(children) => children.get(namespace).copied(),
            NodeContent::Bytes(_) => {
                log::error!("root directory must not be a file");
                return None;
            }
        })
    }

    fn namespace_children_mut(&mut self, namespace: &str) -> Option<&mut NodeChildren> {
        let ns_inode = self.namespace_inode(namespace)?;
        self.inodes.get_mut(&ns_inode)?.children_mut()
    }

    fn create_configmaps_node(&mut self, namespace: &str) {
        let configmaps_inode = self.next_inode();

        let ns_node_children = match self.namespace_children_mut(namespace) {
            Some(children) => children,
            None => {
                log::error!("namespace {namespace} not found or does not contain children");
                return;
            }
        };

        let node_creation_time = SystemTime::now();
        let mut cm_node = Node {
            name: "configmaps".to_string(),
            attrs: FileAttr {
                ino: configmaps_inode,
                size: 0,
                blocks: 0,
                atime: node_creation_time,
                mtime: node_creation_time,
                ctime: node_creation_time,
                crtime: node_creation_time,
                kind: fuser::FileType::Directory,
                perm: 0o755,
                nlink: 2, // FIXME: should be updated when we add children directories
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: BLOCK_SIZE,
            },
            content: NodeContent::Children(NodeChildren::new()),
        };

        ns_node_children.insert("configmaps".to_string(), configmaps_inode);

        match self.core_client.configmaps(namespace).list() {
            Err(e) => {
                log::error!("configmaps fetch failed: {e}");
            }
            Ok(resp) => {
                for item in resp.items.iter() {
                    let name = match item.metadata.name.as_deref() {
                        Some(n) => n,
                        None => continue, // TODO: Should be an error? Should we panic?
                    }
                    .to_owned()
                        + ".yaml";

                    let cm_yaml = serde_yaml::to_string(item).unwrap_or_default().into_bytes();
                    let cm_yaml_size = cm_yaml.len() as u64;
                    let cm_ino = self.next_inode();

                    let cm_creation_time = item
                        .metadata
                        .creation_timestamp
                        .as_ref()
                        .and_then(|t| t.0.timestamp().try_into().ok())
                        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
                        .unwrap_or(UNIX_EPOCH);

                    match &mut cm_node.content {
                        NodeContent::Children(children) => {
                            children.insert(name.to_string(), cm_ino);
                        }
                        NodeContent::Bytes(_) => {
                            log::error!("configmaps directory must not be a file");
                            return;
                        }
                    };
                    self.inodes.insert(
                        cm_ino,
                        Node {
                            name: name.to_string(),
                            attrs: FileAttr {
                                ino: cm_ino,
                                size: cm_yaml_size,
                                blocks: cm_yaml_size.div_ceil(u64::from(BLOCK_SIZE)),
                                atime: cm_creation_time,
                                mtime: cm_creation_time,
                                ctime: cm_creation_time,
                                crtime: cm_creation_time,
                                kind: fuser::FileType::RegularFile,
                                perm: 0o444,
                                nlink: 1,
                                uid: 1000,
                                gid: 1000,
                                rdev: 0,
                                flags: 0,
                                blksize: BLOCK_SIZE,
                            },
                            content: NodeContent::Bytes(cm_yaml),
                        },
                    );
                }
            }
        }

        self.inodes.insert(configmaps_inode, cm_node);
    }
}

impl<'c> fuser::Filesystem for KubeFilesystem<'c> {
    fn init(
        &mut self,
        _req: &fuser::Request<'_>,
        _config: &mut fuser::KernelConfig,
    ) -> Result<(), libc::c_int> {
        let root_node = Node {
            name: "/".to_string(),
            attrs: ROOT_ATTR,
            content: NodeContent::Children(NodeChildren::new()),
        };
        self.inodes.insert(1, root_node);

        match self.core_client.namespaces().list() {
            Err(e) => {
                log::error!("namespaces fetch failed: {e}");
                Err(libc::EIO)
            }
            Ok(resp) => {
                for item in resp.items.iter() {
                    let name = match item.metadata.name.as_deref() {
                        Some(n) => n,
                        None => continue, // TODO: Should be an error? Should we panic?
                    };
                    let ino = self.next_inode();
                    self.create_namespace_node(ino, item);

                    if let Some(root) = self.inodes.get_mut(&1) {
                        match &mut root.content {
                            // TODO: we should check that the file attributes's kind matches the content
                            NodeContent::Children(children) => {
                                children.insert(name.to_string(), ino);
                                root.attrs.nlink += 1; // each child directory increases the link count of the parent
                            }
                            NodeContent::Bytes(_) => {
                                log::error!("root directory must not be a file");
                                return Err(libc::EIO);
                            }
                        }
                    }

                    self.create_configmaps_node(name);
                }
                Ok(())
            }
        }
    }

    fn lookup(
        &mut self,
        _req: &fuser::Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEntry,
    ) {
        log::debug!("lookup parent={parent} name={name:?}\n");
        let child_node = self.inodes.get(&parent).and_then(|p| match &p.content {
            NodeContent::Children(children) => {
                let child_name = name.to_str()?;
                let child_inode = children.get(child_name).copied()?;
                self.inodes.get(&child_inode)
            }
            NodeContent::Bytes(_) => None,
        });

        match child_node {
            Some(n) => reply.entry(&TTL, &n.attrs, 0),
            None => reply.error(libc::ENOENT),
        };
    }

    fn getattr(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        fh: Option<u64>,
        reply: fuser::ReplyAttr,
    ) {
        log::debug!("getattr ino={ino} fh={:?}\n", fh);
        if let Some(node) = self.inodes.get(&ino) {
            return reply.attr(&TTL, &node.attrs);
        } else {
            return reply.error(libc::ENOENT);
        }
    }

    fn readdir(
        &mut self,
        _req: &fuser::Request<'_>,
        inode: u64,
        _fh: u64,
        offset: i64,
        mut reply: fuser::ReplyDirectory,
    ) {
        log::debug!("readdir inode={inode} offset={offset}\n");
        let Some(node) = self.inodes.get(&inode) else {
            reply.error(libc::ENOENT);
            return;
        };

        if node.attrs.kind != fuser::FileType::Directory {
            reply.error(libc::ENOTDIR);
            return;
        }

        let mut entries = vec![
            (inode, fuser::FileType::Directory, "."),
            (1, fuser::FileType::Directory, ".."), // FIXME: should be pointing to the parent inode
        ];

        if let NodeContent::Children(children) = &node.content {
            for (name, &inode) in children.iter() {
                if let Some(child_node) = self.inodes.get(&inode) {
                    entries.push((inode, child_node.attrs.kind, child_node.name.as_str()));
                } else {
                    log::warn!("child {name} with inode {inode} was not found in inodes table");
                }
            }
        } else {
            // TODO: this should probably panic
            reply.error(libc::ENOTDIR);
            return;
        }

        for (i, entry) in entries.into_iter().skip(offset as usize).enumerate() {
            if reply.add(entry.0, (offset + i as i64 + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
        return;
    }

    fn read(
        &mut self,
        _req: &fuser::Request<'_>,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        flags: i32,
        lock_owner: Option<u64>,
        reply: fuser::ReplyData,
    ) {
        log::debug!(
            "read ino={ino} fh={fh} offset={offset} size={size} flags={flags} lock_owner={:?}\n",
            lock_owner
        );
        let Some(node) = self.inodes.get(&ino) else {
            reply.error(libc::ENOENT);
            return;
        };

        if node.attrs.kind != fuser::FileType::RegularFile {
            reply.error(libc::EISDIR);
            return;
        }

        if let NodeContent::Bytes(data) = &node.content {
            let start = offset as usize;
            let end = std::cmp::min(start + size as usize, data.len());
            if start >= data.len() {
                reply.data(&[]);
            } else {
                reply.data(&data[start..end]);
            }
        }
    }

    fn open(&mut self, _req: &fuser::Request<'_>, _ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        // TODO: should at least increase open file handles
        // TODO: only allow RDONLY
        reply.opened(0, 0);
    }

    fn release(
        &mut self,
        _req: &fuser::Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        // should at least release file handles
        reply.ok();
    }
}
