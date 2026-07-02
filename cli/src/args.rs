use clap::Parser;
#[derive(Debug, Parser)]
#[clap(name = "puppynet")]
pub struct Args {
	#[clap(long)]
	pub peer: Vec<String>,
	#[clap(long)]
	pub bind: Vec<String>,
	#[clap(long = "read", value_name = "PATH")]
	pub read: Vec<String>,
	#[clap(long = "write", value_name = "PATH")]
	pub write: Vec<String>,
	#[clap(long, default_value = "0.0.0.0:8832")]
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
	Install {
		#[clap(long)]
		system: bool,
	},
	Start {
		#[clap(long)]
		system: bool,
	},
	Stop {
		#[clap(long)]
		system: bool,
	},
	Restart {
		#[clap(long)]
		system: bool,
	},
	Status {
		#[clap(long)]
		system: bool,
	},
	Uninstall {
		#[clap(long)]
		system: bool,
	},
	Update {
		version: Option<String>,
	},
	CreateUser {
		#[clap(long)]
		username: String,
		#[clap(long)]
		password: String,
	},
	Grant {
		peer_id: String,
		#[clap(long)]
		all: bool,
		#[clap(long = "read", value_name = "PATH")]
		read: Vec<String>,
		#[clap(long = "write", value_name = "PATH")]
		write: Vec<String>,
	},
	Daemon,
}
