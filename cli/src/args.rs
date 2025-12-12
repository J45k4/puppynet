use clap::Parser;
#[derive(Debug, Parser)]
#[clap(name = "puppyagent")]
pub struct Args {
	#[clap(long)]
	pub peer: Vec<String>,
	#[clap(long)]
	pub bind: Vec<String>,
	#[clap(long = "read", value_name = "PATH")]
	pub read: Vec<String>,
	#[clap(long = "write", value_name = "PATH")]
	pub write: Vec<String>,
	#[clap(long, default_value = "127.0.0.1:8832")]
	pub ui_bind: String,
	#[clap(long, value_name = "ADDR")]
	pub http: Option<String>,
	#[clap(subcommand)]
	pub command: Option<Command>,
}

#[derive(Debug, Parser)]
pub enum Command {
	Copy {
		src: String,
		dest: String,
	},
	Scan {
		path: String,
	},
	Install,
	Uninstall,
	Update {
		version: Option<String>,
	},
	CreateUser {
		#[clap(long)]
		username: String,
		#[clap(long)]
		password: String,
	},
	#[cfg(feature = "iced")]
	Gui,
	Daemon,
}
