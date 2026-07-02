#[cfg(feature = "sqlite")]
use std::collections::HashMap;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
#[cfg(feature = "sqlite")]
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use wgui::{schema_diff::diff_schemas, wdb};

#[cfg(feature = "sqlite")]
use wgui::{schema_diff_sql_from_schema_file, write_schema_migration_from_schema_file};

#[derive(Parser, Debug)]
#[command(name = "wgui")]
#[command(about = "WGUI development utilities")]
struct Cli {
	#[command(subcommand)]
	command: TopCommand,
}

#[derive(Subcommand, Debug)]
enum TopCommand {
	Migrations {
		#[command(subcommand)]
		command: MigrationsCommand,
	},
	Migrate {
		#[command(subcommand)]
		command: MigrateCommand,
	},
	Generate(GenerateArgs),
}

#[derive(Subcommand, Debug)]
enum MigrationsCommand {
	New(NewArgs),
	Diff(DiffArgs),
	Create(CreateArgs),
	Compare(CompareArgs),
}

#[derive(Subcommand, Debug)]
enum MigrateCommand {
	Dev(MigrateDevArgs),
}

#[derive(Args, Debug)]
struct NewArgs {
	name: String,
	#[arg(long, default_value = "migrations")]
	dir: PathBuf,
}

#[derive(Args, Debug)]
struct DiffArgs {
	#[arg(long)]
	schema: Option<PathBuf>,
	#[arg(long)]
	db: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct CreateArgs {
	name: String,
	#[arg(long)]
	schema: Option<PathBuf>,
	#[arg(long)]
	db: Option<PathBuf>,
	#[arg(long)]
	dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct CompareArgs {
	#[arg(long)]
	from: PathBuf,
	#[arg(long)]
	to: PathBuf,
}

#[derive(Args, Debug)]
struct MigrateDevArgs {
	#[arg(long)]
	name: String,
	#[arg(long)]
	schema: Option<PathBuf>,
	#[arg(long)]
	migrations_dir: Option<PathBuf>,
	#[arg(long)]
	env_file: Option<PathBuf>,
	#[arg(default_value = ".")]
	project_dir: PathBuf,
}

#[derive(Args, Debug)]
struct GenerateArgs {
	#[arg(default_value = ".")]
	project_dir: PathBuf,
	#[arg(long)]
	schema: Option<PathBuf>,
	#[arg(long)]
	out: Option<PathBuf>,
	#[arg(long)]
	db_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WguiConfig {
	schema: Option<PathBuf>,
	db: Option<PathBuf>,
	out: Option<PathBuf>,
	db_name: Option<String>,
	migrations_dir: Option<PathBuf>,
	#[cfg(feature = "sqlite")]
	env_file: Option<PathBuf>,
}

fn main() {
	if let Err(err) = run() {
		eprintln!("wgui error: {err}");
		std::process::exit(1);
	}
}

fn run() -> Result<(), String> {
	let cli = Cli::parse();
	match cli.command {
		TopCommand::Migrations { command } => run_migrations(command),
		TopCommand::Migrate { command } => run_migrate(command),
		TopCommand::Generate(args) => run_generate(args),
	}
}

fn run_migrations(command: MigrationsCommand) -> Result<(), String> {
	match command {
		MigrationsCommand::New(args) => create_blank_migration(args),
		MigrationsCommand::Diff(args) => diff_migration(args),
		MigrationsCommand::Create(args) => create_schema_migration(args),
		MigrationsCommand::Compare(args) => compare_schemas(args),
	}
}

fn run_migrate(command: MigrateCommand) -> Result<(), String> {
	match command {
		MigrateCommand::Dev(args) => migrate_dev(args),
	}
}

fn run_generate(args: GenerateArgs) -> Result<(), String> {
	let project_dir = resolve_project_dir(&args.project_dir)?;
	let config = load_wgui_config(&project_dir)?;

	let schema_path = resolve_path_with_default(
		args.schema,
		config.schema,
		PathBuf::from("schema.wdb"),
		&project_dir,
	);
	let out_path = resolve_path_with_default(
		args.out,
		config.out,
		PathBuf::from("src/db.rs"),
		&project_dir,
	);
	let db_name = args
		.db_name
		.or(config.db_name)
		.unwrap_or_else(|| "AppDb".to_string());

	let parsed =
		wdb::parse_schema_file(&schema_path).map_err(|e| format!("failed reading schema: {e}"))?;
	let generated = generate_db_rs(&parsed, &db_name)?;

	if let Some(parent) = out_path.parent() {
		std::fs::create_dir_all(parent)
			.map_err(|e| format!("failed creating directory {}: {e}", parent.display()))?;
	}
	std::fs::write(&out_path, generated)
		.map_err(|e| format!("failed writing {}: {e}", out_path.display()))?;
	println!("generated {}", out_path.display());
	Ok(())
}

fn resolve_project_dir(project_dir: &std::path::Path) -> Result<PathBuf, String> {
	std::fs::canonicalize(project_dir).map_err(|e| {
		format!(
			"failed to resolve project dir {}: {e}",
			project_dir.display()
		)
	})
}

fn load_wgui_config(project_dir: &std::path::Path) -> Result<WguiConfig, String> {
	let path = project_dir.join("wgui.toml");
	if !path.exists() {
		return Ok(WguiConfig::default());
	}
	let raw = std::fs::read_to_string(&path)
		.map_err(|e| format!("failed reading {}: {e}", path.display()))?;
	toml::from_str(&raw).map_err(|e| format!("failed parsing {}: {e}", path.display()))
}

fn resolve_path_with_default(
	cli: Option<PathBuf>,
	config: Option<PathBuf>,
	default: PathBuf,
	base: &std::path::Path,
) -> PathBuf {
	let raw = cli.or(config).unwrap_or(default);
	if raw.is_absolute() {
		raw
	} else {
		base.join(raw)
	}
}

fn create_blank_migration(args: NewArgs) -> Result<(), String> {
	let ts = unix_ts()?;
	let filename = format!("{}_{}.sql", ts, normalize_name(&args.name)?);
	let path = args.dir.join(filename);
	let body = format!(
		"-- name: {}\n-- created_at: {}\n\nBEGIN;\n\n-- write migration SQL here\n\nCOMMIT;\n",
		args.name, ts
	);
	write_file(path, body)
}

fn diff_migration(args: DiffArgs) -> Result<(), String> {
	let project_dir = resolve_project_dir(std::path::Path::new("."))?;
	let config = load_wgui_config(&project_dir)?;
	let schema_path = resolve_path_with_default(
		args.schema,
		config.schema,
		PathBuf::from("schema.wdb"),
		&project_dir,
	);
	let db_path =
		resolve_path_with_default(args.db, config.db, PathBuf::from("wgui.db"), &project_dir);

	#[cfg(not(feature = "sqlite"))]
	{
		let _ = (&schema_path, &db_path);
		Err("`wgui migrations diff` requires the `sqlite` feature".to_string())
	}
	#[cfg(feature = "sqlite")]
	{
		let sql = schema_diff_sql_from_schema_file(&schema_path, &db_path)
			.map_err(|e| format!("failed generating schema diff: {e}"))?;
		if let Some(sql) = sql {
			println!("{sql}");
		} else {
			println!("no schema changes");
		}
		Ok(())
	}
}

fn create_schema_migration(args: CreateArgs) -> Result<(), String> {
	let project_dir = resolve_project_dir(std::path::Path::new("."))?;
	let config = load_wgui_config(&project_dir)?;
	let schema_path = resolve_path_with_default(
		args.schema,
		config.schema,
		PathBuf::from("schema.wdb"),
		&project_dir,
	);
	let db_path =
		resolve_path_with_default(args.db, config.db, PathBuf::from("wgui.db"), &project_dir);
	let migrations_dir = resolve_path_with_default(
		args.dir,
		config.migrations_dir,
		PathBuf::from("migrations"),
		&project_dir,
	);

	#[cfg(not(feature = "sqlite"))]
	{
		let _ = (&schema_path, &db_path, &migrations_dir);
		Err("`wgui migrations create` requires the `sqlite` feature".to_string())
	}
	#[cfg(feature = "sqlite")]
	{
		let path = write_schema_migration_from_schema_file(
			&schema_path,
			&db_path,
			&args.name,
			&migrations_dir,
		)
		.map_err(|e| format!("failed creating migration: {e}"))?;
		if let Some(path) = path {
			println!("{}", path.display());
		} else {
			println!("no schema changes");
		}
		Ok(())
	}
}

fn compare_schemas(args: CompareArgs) -> Result<(), String> {
	let from_schema = wdb::parse_schema_file(&args.from)
		.map_err(|e| format!("failed reading --from schema: {e}"))?;
	let to_schema =
		wdb::parse_schema_file(&args.to).map_err(|e| format!("failed reading --to schema: {e}"))?;
	let from_diff = wdb::to_diff_schema(&from_schema);
	let to_diff = wdb::to_diff_schema(&to_schema);
	let ops = diff_schemas(&from_diff, &to_diff);

	if ops.is_empty() {
		println!("no schema changes");
		return Ok(());
	}

	for op in ops {
		match op {
			wgui::schema_diff::DiffOp::CreateTable { table } => {
				println!("create table {} ({})", table.name, table.columns.len());
			}
			wgui::schema_diff::DiffOp::AddColumn { table, column } => {
				println!("add column {}.{}: {}", table, column.name, column.rust_type);
			}
		}
	}
	Ok(())
}

fn migrate_dev(args: MigrateDevArgs) -> Result<(), String> {
	#[cfg(not(feature = "sqlite"))]
	{
		let _ = args;
		Err("`wgui migrate dev` requires the `sqlite` feature".to_string())
	}
	#[cfg(feature = "sqlite")]
	{
		if args.name.trim().is_empty() {
			return Err("migration name cannot be empty".to_string());
		}

		let project_dir = resolve_project_dir(&args.project_dir)?;
		let config = load_wgui_config(&project_dir)?;
		let env_path = resolve_path_with_default(
			args.env_file,
			config.env_file,
			PathBuf::from(".env"),
			&project_dir,
		);
		let schema_path = resolve_path_with_default(
			args.schema,
			config.schema,
			PathBuf::from("schema.wdb"),
			&project_dir,
		);
		let migrations_dir = resolve_path_with_default(
			args.migrations_dir,
			config.migrations_dir,
			PathBuf::from("migrations"),
			&project_dir,
		);

		if !schema_path.exists() {
			return Err(format!("schema file not found: {}", schema_path.display()));
		}
		if !env_path.exists() {
			return Err(format!(".env file not found: {}", env_path.display()));
		}

		let envs = read_env_file(&env_path)?;
		let database_url = envs
			.get("DATABASE_URL")
			.or_else(|| envs.get("WGUI_DATABASE_URL"))
			.ok_or_else(|| {
				format!(
					"DATABASE_URL not found in {} (or WGUI_DATABASE_URL)",
					env_path.display()
				)
			})?;
		let db_path = resolve_database_path(database_url, &project_dir)?;

		if let Some(parent) = db_path.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("failed creating db directory {}: {e}", parent.display()))?;
		}
		std::fs::create_dir_all(&migrations_dir).map_err(|e| {
			format!(
				"failed creating migrations dir {}: {e}",
				migrations_dir.display()
			)
		})?;

		let conn = Connection::open(&db_path)
			.map_err(|e| format!("failed opening sqlite database {}: {e}", db_path.display()))?;
		ensure_applied_migrations_table(&conn)?;

		let mut applied_any = false;
		for migration in list_sql_migrations(&migrations_dir)? {
			if is_migration_applied(&conn, &migration)? {
				continue;
			}
			let migration_path = migrations_dir.join(&migration);
			let sql = std::fs::read_to_string(&migration_path).map_err(|e| {
				format!("failed reading migration {}: {e}", migration_path.display())
			})?;
			conn.execute_batch(&sql)
				.map_err(|e| format!("failed applying migration {}: {e}", migration))?;
			mark_migration_applied(&conn, &migration)?;
			println!("applied migration {}", migration);
			applied_any = true;
		}

		let created = write_schema_migration_from_schema_file(
			&schema_path,
			&db_path,
			&args.name,
			&migrations_dir,
		)
		.map_err(|e| format!("failed creating schema migration: {e}"))?;

		if let Some(path) = created {
			let filename = path
				.file_name()
				.and_then(|s| s.to_str())
				.ok_or_else(|| format!("invalid migration file name: {}", path.display()))?
				.to_string();
			let sql = std::fs::read_to_string(&path)
				.map_err(|e| format!("failed reading migration {}: {e}", path.display()))?;
			conn.execute_batch(&sql)
				.map_err(|e| format!("failed applying migration {}: {e}", path.display()))?;
			mark_migration_applied(&conn, &filename)?;
			println!("created and applied {}", path.display());
			applied_any = true;
		}

		if !applied_any {
			println!("database is up to date");
		}
		Ok(())
	}
}

fn write_file(path: PathBuf, body: String) -> Result<(), String> {
	let parent = path
		.parent()
		.ok_or_else(|| format!("invalid migration path {}", path.display()))?;
	std::fs::create_dir_all(parent)
		.map_err(|e| format!("failed creating directory {}: {e}", parent.display()))?;
	std::fs::write(&path, body).map_err(|e| format!("failed writing {}: {e}", path.display()))?;
	println!("{}", path.display());
	Ok(())
}

fn unix_ts() -> Result<u64, String> {
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map_err(|e| format!("system clock error: {e}"))?;
	Ok(now.as_secs())
}

fn normalize_name(raw: &str) -> Result<String, String> {
	let mut out = String::new();
	for ch in raw.chars() {
		if ch.is_ascii_alphanumeric() {
			out.push(ch.to_ascii_lowercase());
		} else if ch == '-' || ch == '_' || ch == ' ' {
			if !out.ends_with('_') {
				out.push('_');
			}
		}
	}
	let out = out.trim_matches('_').to_string();
	if out.is_empty() {
		return Err("migration name must contain letters or numbers".to_string());
	}
	Ok(out)
}

fn generate_db_rs(schema: &wgui::wdb::SchemaAst, db_name: &str) -> Result<String, String> {
	if db_name.trim().is_empty() {
		return Err("db_name cannot be empty".to_string());
	}
	let mut out = String::new();
	out.push_str("use wgui::{Db, DbTable, HasId, Wdb, WguiModel};\n\n");

	let mut table_inits: Vec<(String, String)> = Vec::new();
	let mut db_fields: Vec<(String, String)> = Vec::new();

	for model in &schema.models {
		let model_name = &model.name;
		let struct_fields = model
			.fields
			.iter()
			.filter(|f| !f.attributes.iter().any(|a| a.name == "relation"))
			.collect::<Vec<_>>();

		out.push_str("#[derive(Debug, Clone, WguiModel, serde::Serialize, serde::Deserialize)]\n");
		out.push_str(&format!("pub struct {} {{\n", model_name));
		for field in &struct_fields {
			let ty = wdb_type_to_rust(&field.ty);
			out.push_str(&format!("\tpub {}: {},\n", field.name, ty));
		}
		out.push_str("}\n\n");

		let has_id_u32 = struct_fields
			.iter()
			.find(|f| f.name == "id")
			.map(|f| f.ty.name == "Int" || f.ty.name == "u32")
			.unwrap_or(false);
		if has_id_u32 {
			out.push_str(&format!("impl HasId for {} {{\n", model_name));
			out.push_str("\tfn id(&self) -> u32 {\n\t\tself.id\n\t}\n\n");
			out.push_str("\tfn set_id(&mut self, id: u32) {\n\t\tself.id = id;\n\t}\n");
			out.push_str("}\n\n");
		}

		let table_field = pluralize(&to_snake_case(model_name));
		db_fields.push((table_field.clone(), model_name.clone()));
		let init = format!("{}: db.table()", table_field);
		table_inits.push((table_field, init));
	}

	out.push_str("#[derive(Debug, Wdb)]\n");
	out.push_str(&format!("pub struct {} {{\n", db_name));
	for (field_name, model_name) in &db_fields {
		out.push_str(&format!("\tpub {}: DbTable<{}>,\n", field_name, model_name));
	}
	out.push_str("}\n\n");

	out.push_str(&format!("impl {} {{\n", db_name));
	out.push_str("\tpub fn new() -> Self {\n");
	out.push_str(&format!("\t\tlet db = Db::<{}>::new();\n", db_name));
	out.push_str("\t\tSelf {\n");
	for (_, init) in &table_inits {
		out.push_str(&format!("\t\t\t{},\n", init));
	}
	out.push_str("\t\t}\n");
	out.push_str("\t}\n");
	out.push_str("}\n");

	Ok(out)
}

fn wdb_type_to_rust(ty: &wgui::wdb::TypeAst) -> String {
	let base = match ty.name.as_str() {
		"Bool" => "bool".to_string(),
		"String" => "String".to_string(),
		"Int" => "u32".to_string(),
		"BigInt" => "i64".to_string(),
		"Float" | "Decimal" => "f64".to_string(),
		"UUID" | "DateTime" | "Json" | "Bytes" => "String".to_string(),
		other => other.to_string(),
	};
	let with_list = if ty.is_list {
		format!("Vec<{}>", base)
	} else {
		base
	};
	if ty.is_optional {
		format!("Option<{}>", with_list)
	} else {
		with_list
	}
}

fn to_snake_case(input: &str) -> String {
	let mut out = String::new();
	for (idx, ch) in input.chars().enumerate() {
		if ch.is_ascii_uppercase() {
			if idx > 0 {
				out.push('_');
			}
			out.push(ch.to_ascii_lowercase());
		} else {
			out.push(ch);
		}
	}
	out
}

fn pluralize(singular: &str) -> String {
	if singular.ends_with('s') {
		format!("{}es", singular)
	} else {
		format!("{}s", singular)
	}
}

#[cfg(feature = "sqlite")]
fn read_env_file(path: &std::path::Path) -> Result<HashMap<String, String>, String> {
	let raw = std::fs::read_to_string(path)
		.map_err(|e| format!("failed reading env file {}: {e}", path.display()))?;
	let mut out = HashMap::new();
	for line in raw.lines() {
		let line = line.trim();
		if line.is_empty() || line.starts_with('#') {
			continue;
		}
		let Some((k, v)) = line.split_once('=') else {
			continue;
		};
		let key = k.trim().to_string();
		let mut value = v.trim().to_string();
		if (value.starts_with('"') && value.ends_with('"'))
			|| (value.starts_with('\'') && value.ends_with('\''))
		{
			value = value[1..value.len() - 1].to_string();
		}
		out.insert(key, value);
	}
	Ok(out)
}

#[cfg(feature = "sqlite")]
fn resolve_database_path(url: &str, project_dir: &std::path::Path) -> Result<PathBuf, String> {
	if let Some(rest) = url.strip_prefix("sqlite://") {
		if rest.starts_with('/') {
			return Ok(PathBuf::from(rest));
		}
		return Ok(project_dir.join(rest));
	}
	if let Some(rest) = url.strip_prefix("sqlite:") {
		if rest == ":memory:" {
			return Err("sqlite :memory: is not supported for `migrate dev`".to_string());
		}
		if rest.starts_with('/') {
			return Ok(PathBuf::from(rest));
		}
		return Ok(project_dir.join(rest));
	}
	if url.contains("://") {
		let scheme = url.split("://").next().unwrap_or("unknown");
		return Err(format!(
			"database scheme `{scheme}` is not supported yet; currently only sqlite URLs are supported"
		));
	}
	Ok(project_dir.join(url))
}

#[cfg(feature = "sqlite")]
fn ensure_applied_migrations_table(conn: &Connection) -> Result<(), String> {
	conn.execute(
		"CREATE TABLE IF NOT EXISTS _wgui_migrations (\n\
\tfilename TEXT PRIMARY KEY,\n\
\tapplied_at INTEGER NOT NULL\n\
)",
		[],
	)
	.map_err(|e| format!("failed creating _wgui_migrations: {e}"))?;
	Ok(())
}

#[cfg(feature = "sqlite")]
fn is_migration_applied(conn: &Connection, filename: &str) -> Result<bool, String> {
	let found: Option<String> = conn
		.query_row(
			"SELECT filename FROM _wgui_migrations WHERE filename = ?1",
			params![filename],
			|row| row.get(0),
		)
		.optional()
		.map_err(|e| format!("failed checking migration state for {filename}: {e}"))?;
	Ok(found.is_some())
}

#[cfg(feature = "sqlite")]
fn mark_migration_applied(conn: &Connection, filename: &str) -> Result<(), String> {
	conn.execute(
		"INSERT OR REPLACE INTO _wgui_migrations (filename, applied_at) VALUES (?1, unixepoch())",
		params![filename],
	)
	.map_err(|e| format!("failed marking migration {filename} as applied: {e}"))?;
	Ok(())
}

#[cfg(feature = "sqlite")]
fn list_sql_migrations(dir: &std::path::Path) -> Result<Vec<String>, String> {
	if !dir.exists() {
		return Ok(Vec::new());
	}
	let mut files = Vec::new();
	for entry in
		std::fs::read_dir(dir).map_err(|e| format!("failed reading {}: {e}", dir.display()))?
	{
		let entry = entry.map_err(|e| format!("failed reading dir entry: {e}"))?;
		let path = entry.path();
		if path.extension().and_then(|s| s.to_str()) != Some("sql") {
			continue;
		}
		let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
			continue;
		};
		files.push(name.to_string());
	}
	files.sort();
	Ok(files)
}
