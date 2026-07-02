use std::sync::Mutex;

pub trait HasId {
	fn id(&self) -> u32;
	fn set_id(&mut self, id: u32);
}

#[derive(Debug)]
pub struct Table<T> {
	rows: Mutex<Vec<T>>,
	next_id: Mutex<u32>,
}

impl<T> Table<T> {
	pub fn new(rows: Vec<T>) -> Self {
		Self {
			rows: Mutex::new(rows),
			next_id: Mutex::new(1),
		}
	}

	pub fn snapshot(&self) -> Vec<T>
	where
		T: Clone,
	{
		self.rows.lock().unwrap().clone()
	}

	pub fn replace(&self, rows: Vec<T>) {
		*self.rows.lock().unwrap() = rows;
	}

	pub async fn insert(&self, row: T) {
		self.rows.lock().unwrap().push(row);
	}
}

impl<T> Table<T>
where
	T: HasId + Clone,
{
	pub fn with_ids(rows: Vec<T>) -> Self {
		let next_id = rows
			.iter()
			.map(HasId::id)
			.max()
			.unwrap_or(0)
			.saturating_add(1);
		Self {
			rows: Mutex::new(rows),
			next_id: Mutex::new(next_id),
		}
	}

	pub fn next_id(&self) -> u32 {
		let mut next_id = self.next_id.lock().unwrap();
		let id = *next_id;
		*next_id = next_id.saturating_add(1);
		id
	}

	pub async fn save(&self, mut row: T) -> T {
		if row.id() == 0 {
			row.set_id(self.next_id());
		}
		self.rows.lock().unwrap().push(row.clone());
		row
	}

	pub async fn find(&self, id: u32) -> Option<T> {
		self.rows
			.lock()
			.unwrap()
			.iter()
			.find(|row| row.id() == id)
			.cloned()
	}
}
