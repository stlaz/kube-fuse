use std::{
    collections::HashMap,
    time::{Duration, UNIX_EPOCH},
};

use fuser::{self, FileAttr};
use libc;

use client_rs::{corev1::CoreV1Client, rest};

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
    blksize: 512,
};

const TTL: Duration = Duration::from_secs(1);

type InodeTable = HashMap<u64, Node>;
type NodeChildren = HashMap<String, u64>;
struct Node {
    name: String,
    attrs: FileAttr,
    children: Option<NodeChildren>,
}
pub struct KubeFilesystem<'c> {
    // Add fields as necessary
    core_client: CoreV1Client<'c>,

    inodes: InodeTable,
}

impl<'c> KubeFilesystem<'c> {
    pub fn new(rest_client: &'c rest::RestClient) -> Self {
        KubeFilesystem {
            core_client: CoreV1Client::new(rest_client),

            inodes: InodeTable::new(),
        }
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
            children: Some(NodeChildren::new()),
        };
        self.inodes.insert(1, root_node);

        match self.core_client.namespaces().list() {
            Ok(resp) => {
                for (i, item) in resp.items.iter().enumerate() {
                    let name = match item.metadata.name.as_deref() {
                        Some(n) => n,
                        None => continue, // TODO: Should be an error? Should we panic?
                    };
                    let ino = 2 + i as u64; // FIXME: stable inode generation?
                    let ns_node = Node {
                        name: name.to_string(),
                        attrs: FileAttr {
                            ino,
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
                            blksize: 512,
                        },
                        children: None,
                    };
                    self.inodes.insert(ino, ns_node);

                    if let Some(root) = self.inodes.get_mut(&1) {
                        if let Some(children) = root.children.as_mut() {
                            children.insert(name.to_string(), ino);
                        }
                    }
                }
                Ok(())
            }
            Err(e) => {
                log::error!("namespaces fetch failed: {e}");
                Err(libc::EIO)
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
        let child_node = match self.inodes.get(&parent).and_then(|p| {
            p.children
                .as_ref()
                .and_then(|children| {
                    let child_name = name.to_str()?;
                    children.get(child_name).copied()
                })
                .and_then(|inode| self.inodes.get(&inode))
        }) {
            Some(n) => n,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        reply.entry(&TTL, &child_node.attrs, 0);
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

        let mut entries = vec![
            (inode, fuser::FileType::Directory, "."),
            (1, fuser::FileType::Directory, ".."), // FIXME: should be pointing to the parent inode
        ];

        if let Some(children) = node.children.as_ref() {
            for (name, &inode) in children.iter() {
                if let Some(child_node) = self.inodes.get(&inode) {
                    entries.push((inode, child_node.attrs.kind, child_node.name.as_str()));
                } else {
                    log::warn!("child {name} with inode {inode} was not found in inodes table");
                }
            }
        }

        for (i, entry) in entries.into_iter().skip(offset as usize).enumerate() {
            if reply.add(entry.0, (offset + i as i64 + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
        return;
    }
}
