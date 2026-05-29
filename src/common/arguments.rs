// Vendored and adapted from airframesio/xng (src/common/arguments.rs, GPL-3.0-or-later).
// Trimmed to the args this binary actually uses (listen-host/port, disable-cross-site,
// quiet/verbose) and wired with env() fallbacks so existing deployments keep working.

use clap::{Arg, ArgAction, ArgMatches, Command, arg, value_parser};

pub fn register_common_arguments(cmd: Command) -> Command {
    cmd.args(&[
        Arg::new("listen-host")
            .long("listen-host")
            .env("TAP_LISTEN_HOST")
            .value_name("HOST")
            .help("Interface to bind the HTTP control/stats API on"),
        Arg::new("listen-port")
            .long("listen-port")
            .env("TAP_LISTEN_PORT")
            .value_name("PORT")
            .value_parser(value_parser!(u16))
            .help("Port to bind the HTTP control/stats API on (default: 7871)"),
        arg!(--"disable-cross-site" "Restrict CORS Access-Control-Allow-Origin to the bound listener"),
        Arg::new("quiet")
            .long("quiet")
            .short('q')
            .action(ArgAction::SetTrue)
            .help("Suppress informational output"),
        Arg::new("verbose")
            .long("verbose")
            .short('v')
            .action(ArgAction::Count)
            .help("Increase log verbosity (-v debug, -vv trace)"),
    ])
}

pub fn parse_listen_host(args: &ArgMatches, default: &str) -> String {
    args.get_one::<String>("listen-host")
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

pub fn parse_listen_port(args: &ArgMatches, default: u16) -> u16 {
    args.get_one::<u16>("listen-port")
        .copied()
        .unwrap_or(default)
}

pub fn parse_disable_cross_site(args: &ArgMatches) -> bool {
    args.get_flag("disable-cross-site")
}
