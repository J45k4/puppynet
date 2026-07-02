#[cfg(not(feature = "sqlite"))]
use crate::table::Table;
use crate::wui::runtime::{WdbModel, WdbSchema};
#[cfg(feature = "sqlite")]
use crate::{SQLLiteDB, SqliteTable};
#[cfg(feature = "sqlite")]
use std::path::{Path, PathBuf};

pub struct DbTable<T> {
	#[cfg(feature = "sqlite")]
	inner: SqliteTable<T>,
	#[cfg(not(feature = "sqlite"))]
	inner: Table<T>,
}

pub struct Db<S: WdbSchema> {
	#[cfg(feature = "sqlite")]
	sqlite: SQLLiteDB<S>,
	_schema: std::marker::PhantomData<S>,
}

#[cfg(feature = "sqlite")]
pub trait DbSerdeBounds: serde::Serialize + serde::de::DeserializeOwned {}

#[cfg(feature = "sqlite")]
impl<T> DbSerdeBounds for T where T: serde::Serialize + serde::de::DeserializeOwned {}

#[cfg(not(feature = "sqlite"))]
pub trait DbSerdeBounds {}

#[cfg(not(feature = "sqlite"))]
impl<T> DbSerdeBounds for T {}

impl<T> std::fmt::Debug for DbTable<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		#[cfg(feature = "sqlite")]
		{
			return f.write_str("DbTable<sqlite>");
		}
		#[cfg(not(feature = "sqlite"))]
		{
			f.write_str("DbTable<memory>")
		}
	}
}

impl<T> DbTable<T> {
	#[cfg(feature = "sqlite")]
	pub fn from_sqlite(inner: SqliteTable<T>) -> Self {
		Self { inner }
	}

	#[cfg(not(feature = "sqlite"))]
	pub fn new(rows: Vec<T>) -> Self {
		Self {
			inner: Table::new(rows),
		}
	}

	#[cfg(not(feature = "sqlite"))]
	pub fn with_ids(rows: Vec<T>) -> Self
	where
		T: crate::HasId + Clone,
	{
		Self {
			inner: Table::with_ids(rows),
		}
	}

	pub fn snapshot(&self) -> Vec<T>
	where
		T: WdbModel + Clone + DbSerdeBounds,
	{
		#[cfg(feature = "sqlite")]
		{
			self.inner.snapshot_sync().expect("sqlite snapshot failed")
		}
		#[cfg(not(feature = "sqlite"))]
		{
			self.inner.snapshot()
		}
	}

	pub fn replace(&self, rows: Vec<T>)
	where
		T: WdbModel + Clone + DbSerdeBounds,
	{
		#[cfg(feature = "sqlite")]
		{
			self.inner
				.replace_sync(rows)
				.expect("sqlite replace failed");
		}
		#[cfg(not(feature = "sqlite"))]
		{
			self.inner.replace(rows);
		}
	}

	pub async fn insert(&self, row: T)
	where
		T: WdbModel + Clone + DbSerdeBounds,
	{
		#[cfg(feature = "sqlite")]
		{
			self.inner.insert(row).await.expect("sqlite insert failed");
		}
		#[cfg(not(feature = "sqlite"))]
		{
			self.inner.insert(row).await;
		}
	}
}

impl<T> DbTable<T>
where
	T: WdbModel + crate::HasId + Clone + DbSerdeBounds,
{
	pub fn next_id(&self) -> u32 {
		#[cfg(feature = "sqlite")]
		{
			self.inner.next_id_sync().expect("sqlite next_id failed")
		}
		#[cfg(not(feature = "sqlite"))]
		{
			self.inner.next_id()
		}
	}

	pub async fn save(&self, row: T) -> T {
		#[cfg(feature = "sqlite")]
		{
			self.inner.save(row).await.expect("sqlite save failed")
		}
		#[cfg(not(feature = "sqlite"))]
		{
			self.inner.save(row).await
		}
	}

	pub async fn find(&self, id: u32) -> Option<T> {
		#[cfg(feature = "sqlite")]
		{
			self.inner.find(id).await.expect("sqlite find failed")
		}
		#[cfg(not(feature = "sqlite"))]
		{
			self.inner.find(id).await
		}
	}
}

impl<T> WdbModel for DbTable<T>
where
	T: WdbModel,
{
	fn schema() -> crate::wui::runtime::WdbModelSchema {
		T::schema()
	}
}

impl<S: WdbSchema> Db<S> {
	pub fn new() -> Self {
		#[cfg(feature = "sqlite")]
		{
			return Self {
				sqlite: SQLLiteDB::<S>::new().expect("open sqlite db"),
				_schema: std::marker::PhantomData,
			};
		}
		#[cfg(not(feature = "sqlite"))]
		{
			Self {
				_schema: std::marker::PhantomData,
			}
		}
	}

	pub fn table<T>(&self) -> DbTable<T>
	where
		T: WdbModel + Clone + DbSerdeBounds,
	{
		#[cfg(feature = "sqlite")]
		{
			return DbTable::from_sqlite(
				self.sqlite.table::<T>().expect("create/open sqlite table"),
			);
		}
		#[cfg(not(feature = "sqlite"))]
		{
			DbTable::new(Vec::new())
		}
	}

	pub fn table_with_ids<T>(&self, rows: Vec<T>) -> DbTable<T>
	where
		T: WdbModel + crate::HasId + Clone + DbSerdeBounds,
	{
		#[cfg(feature = "sqlite")]
		{
			let table =
				DbTable::from_sqlite(self.sqlite.table::<T>().expect("create/open sqlite table"));
			if table
				.inner
				.row_count_sync()
				.expect("count sqlite table rows failed")
				== 0
			{
				table.replace(rows);
			}
			return table;
		}
		#[cfg(not(feature = "sqlite"))]
		{
			DbTable::with_ids(rows)
		}
	}

	#[cfg(feature = "sqlite")]
	pub fn migration_sql_for_path<P: AsRef<Path>>(db_path: P) -> anyhow::Result<Option<String>> {
		crate::schema_diff_sql::<S, _>(db_path)
	}

	#[cfg(feature = "sqlite")]
	pub fn create_migration_for_path<P: AsRef<Path>, Q: AsRef<Path>>(
		db_path: P,
		name: &str,
		dir: Q,
	) -> anyhow::Result<Option<PathBuf>> {
		crate::write_schema_migration::<S, _, _>(db_path, name, dir)
	}
}
