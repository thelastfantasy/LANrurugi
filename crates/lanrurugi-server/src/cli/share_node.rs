//! `lanrurugi-server share-node`: a sharing rendezvous (and optionally relay) node with no library
//! and no Redis, for a small VPS that friends' instances list as a node (017 FR-054).

use std::path::PathBuf;

#[derive(clap::Args)]
pub struct ShareNodeArgs {
    /// Directory for the node key and the rendezvous packets kept across restarts.
    #[arg(
        long,
        env = "LANRURUGI_SHARE_NODE_DATA",
        default_value = "./share-node"
    )]
    pub data: PathBuf,
    /// UDP port the node listens on; publish it as 443/udp where possible.
    #[arg(long, default_value_t = 11204)]
    pub udp_port: u16,
    /// The host name or address other instances reach this node at, printed in its node string.
    #[arg(long)]
    pub public_host: Option<String>,
    /// Also run a relay. It listens on `--relay-port` (TCP) and, with `acme` or `files`, on
    /// 7842/udp for address discovery, which lets peers behind NAT hole-punch.
    #[arg(long)]
    pub relay: bool,
    /// The relay's TCP port.
    #[arg(long, default_value_t = 443)]
    pub relay_port: u16,
    /// `acme` (Let's Encrypt for `--public-host`), `files` (`--relay-cert`/`--relay-key`) or
    /// `proxy` (plain HTTP on localhost behind a TLS-terminating tunnel or reverse proxy).
    #[arg(long, default_value = "acme")]
    pub relay_tls: String,
    #[arg(long)]
    pub relay_cert: Option<PathBuf>,
    #[arg(long)]
    pub relay_key: Option<PathBuf>,
    /// PEM roots to trust in addition to the bundled ones, for a relay certificate from a private CA.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
    /// Per-client relay rate in bytes per second (default 10 MiB/s); 0 for no limit.
    #[arg(long)]
    pub relay_rate: Option<u64>,
    /// An endpoint id or node string the relay admits; repeat for more. Without any, it admits
    /// everyone.
    #[arg(long)]
    pub relay_allow: Vec<String>,
}

pub async fn run(args: ShareNodeArgs) -> anyhow::Result<()> {
    lanrurugi_api::share_net::nodes::run_standalone(lanrurugi_api::share_net::nodes::NodeOptions {
        data_dir: args.data,
        udp_port: args.udp_port,
        public_host: args.public_host,
        relay: args.relay,
        relay_port: args.relay_port,
        relay_tls: args.relay_tls,
        relay_cert: args.relay_cert,
        relay_key: args.relay_key,
        ca_file: args.ca_file,
        relay_rate: args.relay_rate,
        relay_allow: args.relay_allow,
    })
    .await
    .map_err(anyhow::Error::msg)
}
