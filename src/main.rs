mod kubefuse;

use client_rs::rest;

use clap::Parser;

use crate::kubefuse::KubeFilesystem;

#[derive(Parser, Debug)]
struct Options {
    #[arg(short, long)]
    cluster_url: String,

    #[arg(short, long, env = "KUBE_TOKEN")]
    token: String,

    #[arg(short, long)]
    mountpoint: String,
}

fn main() {
    env_logger::init();
    log::info!("starting");

    let opts = Options::parse();
    let rest_client = rest::rest_client_for(&rest::Config {
        base_url: opts.cluster_url.to_string(),
        user_agent: None,
        bearer_token: opts.token.to_string().into(),
    });

    let fs = KubeFilesystem::new(&rest_client);
    fuser::mount2(fs, opts.mountpoint, &[]).unwrap();
}
