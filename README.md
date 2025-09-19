# Inspecting your Kubernetes cluster using filesystem commands

This experimental, learning repository allows a user to introspect their Kubernetes
cluster as if they were working with their files and directories.

## Requirements

- Run on a system that is capable of mounting FUSE filesystems

## Running it

The solution is a mere PoC and requires broad permissions to be able to view objects
all across the cluster.

To run it:
```bash
KUBE_TOKEN=KUBE_TOKEN=$(k create token -n <SA_NAMESPACE> <SA_NAME>) # use a SA with broad permissions
kube-fuse --cluster-url <kube-apiserver-url> -t $KUBE_TOKEN -m <mount-path>
```

This will start the binary and mount your cluster's resources as directories and
files at `<mount-path`>.

You can then run things like this:
```bash
$ tree /tmp/kubefuse-test/1/
/tmp/kubefuse-test/1/
├── default
│   ├── configmaps
│   │   └── kube-root-ca.crt.yaml
│   └── manifest.yaml
├── kube-node-lease
│   ├── configmaps
│   │   └── kube-root-ca.crt.yaml
│   └── manifest.yaml
├── kube-public
│   ├── configmaps
│   │   ├── cluster-info.yaml
│   │   └── kube-root-ca.crt.yaml
│   └── manifest.yaml
├── kube-system
│   ├── configmaps
│   │   ├── coredns.yaml
│   │   ├── extension-apiserver-authentication.yaml
│   │   ├── kubeadm-config.yaml
│   │   ├── kube-apiserver-legacy-service-account-token-tracking.yaml
│   │   ├── kubelet-config.yaml
│   │   ├── kube-proxy.yaml
│   │   └── kube-root-ca.crt.yaml
│   └── manifest.yaml
└── local-path-storage
    ├── configmaps
    │   ├── kube-root-ca.crt.yaml
    │   └── local-path-config.yaml
    └── manifest.yaml

11 directories, 18 files
```

After interrupting/killing the main process, run the following to unmount the fs
cleanly:
```bash
fusermount3 -u <mount-path>
```

## Known issues

- currently only the snapshot of the cluster at the time of the start of the binary
  is presented - the records do not update
- only namespaces and configmaps are currently presented
- no writes are currently possible as the client doesn't currently implement Updates
- uses a token from command line instead of a file to be able to reload it
- the code is a bit of a mess right now, I don't know Rust well and I was rushing
  to have at least basic functionality done 🙂
