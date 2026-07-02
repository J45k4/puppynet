use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct PubSub<T>
where
	T: Clone + Send + Sync + 'static,
{
	topics: Arc<Mutex<HashMap<String, broadcast::Sender<T>>>>,
	capacity: usize,
}

impl<T> PubSub<T>
where
	T: Clone + Send + Sync + 'static,
{
	pub fn new() -> Self {
		Self::with_capacity(32)
	}

	pub fn with_capacity(capacity: usize) -> Self {
		Self {
			topics: Arc::new(Mutex::new(HashMap::new())),
			capacity,
		}
	}

	pub fn subscribe(&self, topic: &str) -> broadcast::Receiver<T> {
		let mut topics = self.topics.lock().unwrap();
		let sender = topics.entry(topic.to_string()).or_insert_with(|| {
			let (sender, _) = broadcast::channel(self.capacity);
			sender
		});
		sender.subscribe()
	}

	pub fn publish(&self, topic: &str, value: T) {
		let sender = {
			let mut topics = self.topics.lock().unwrap();
			topics
				.entry(topic.to_string())
				.or_insert_with(|| {
					let (sender, _) = broadcast::channel(self.capacity);
					sender
				})
				.clone()
		};
		let _ = sender.send(value);
	}
}
