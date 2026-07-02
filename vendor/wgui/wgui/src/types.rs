use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use crate::gui::{Item, ThreeKind, ThreeProp, ThreePropValue};

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub struct OnClick {
	pub id: u32,
	pub inx: Option<u32>,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnKeyDown {
	pub id: Option<String>,
	pub keycode: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnTextChanged {
	pub id: u32,
	pub inx: Option<u32>,
	pub value: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct PathChanged {
	pub path: String,
	pub query: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct InputQuery {}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnSliderChange {
	pub id: u32,
	pub inx: Option<u32>,
	pub value: i32,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnSelect {
	pub id: u32,
	pub inx: Option<u32>,
	pub value: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnCustom {
	pub id: u32,
	pub inx: Option<u32>,
	pub name: String,
	pub payload: serde_json::Value,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcJoin {
	pub room: String,
	pub audio: bool,
	pub video: bool,
	#[serde(alias = "display_name")]
	pub display_name: Option<String>,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcParticipant {
	pub client_id: usize,
	pub display_name: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcLeave {
	pub room: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcSignal {
	pub room: String,
	#[serde(alias = "target_client_id")]
	pub target_client_id: Option<usize>,
	pub payload: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebPushSubscriptionChanged {
	#[serde(default)]
	pub subscription: Option<serde_json::Value>,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ClientEvent {
	Disconnected { id: usize },
	Connected { id: usize },
	PathChanged(PathChanged),
	Input(InputQuery),
	OnClick(OnClick),
	OnTextChanged(OnTextChanged),
	OnSliderChange(OnSliderChange),
	OnSelect(OnSelect),
	OnCustom(OnCustom),
	WebRtcJoin(WebRtcJoin),
	WebRtcLeave(WebRtcLeave),
	WebRtcSignal(WebRtcSignal),
	WebPushSubscriptionChanged(WebPushSubscriptionChanged),
}

#[derive(Debug, Clone)]
pub struct ClientMessage {
	pub client_id: usize,
	pub event: ClientEvent,
}

pub type ItemPath = Vec<usize>;

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Replace {
	pub path: ItemPath,
	pub item: Item,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplaceAt {
	pub path: ItemPath,
	pub item: Item,
	pub inx: usize,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct AddBack {
	pub path: ItemPath,
	pub item: Item,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct AddFront {
	pub path: ItemPath,
	pub item: Item,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct InsertAt {
	pub path: ItemPath,
	pub item: Item,
	pub inx: usize,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoveInx {
	pub path: ItemPath,
	pub inx: usize,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct PushState {
	pub url: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Navigate {
	pub url: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplaceState {
	pub url: String,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetQuery {
	pub query: HashMap<String, String>,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum Value {
	String(String),
	Number(u32),
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub enum PropKey {
	ID = 1,
	Border = 2,
	BackgroundColor = 3,
	Spacing = 4,
	FlexDirection = 5,
	Grow = 6,
	Width = 7,
	Height = 8,
	MinWidth = 9,
	MaxWidth = 10,
	MinHeight = 11,
	MaxHeight = 12,
	Padding = 13,
	Overflow = 14,
	BreakWords = 15,
	Fill = 16,
	Color = 17,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetProp {
	pub key: PropKey,
	pub value: Value,
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreeOp {
	Create {
		id: u32,
		kind: ThreeKind,
		props: Vec<ThreeProp>,
	},
	Attach {
		#[serde(rename = "parentId")]
		parent_id: u32,
		#[serde(rename = "childId")]
		child_id: u32,
	},
	Detach {
		#[serde(rename = "parentId")]
		parent_id: u32,
		#[serde(rename = "childId")]
		child_id: u32,
	},
	SetProp {
		id: u32,
		key: String,
		value: ThreePropValue,
	},
	UnsetProp {
		id: u32,
		key: String,
	},
	Delete {
		id: u32,
	},
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ClientAction {
	Replace(Replace),
	ReplaceAt(ReplaceAt),
	AddBack(AddBack),
	AddFront(AddFront),
	InsertAt(InsertAt),
	RemoveInx(RemoveInx),
	PushState(PushState),
	Navigate(Navigate),
	ReplaceState(ReplaceState),
	SetQuery(SetQuery),
	SetProp {
		path: ItemPath,
		sets: Vec<SetProp>,
	},
	ThreePatch {
		path: ItemPath,
		ops: Vec<ThreeOp>,
	},
	SetTitle {
		title: String,
	},
	WebRtcRoomState {
		room: String,
		#[serde(rename = "selfClientId")]
		self_client_id: usize,
		peers: Vec<usize>,
		participants: Vec<WebRtcParticipant>,
	},
	WebRtcSignal {
		room: String,
		#[serde(rename = "fromClientId")]
		from_client_id: usize,
		payload: String,
	},
	WebPushEnable {
		#[serde(rename = "serviceWorkerPath")]
		service_worker_path: String,
		#[serde(rename = "vapidPublicKey")]
		vapid_public_key: Option<String>,
	},
	WebPushDisable {
		#[serde(rename = "serviceWorkerPath")]
		service_worker_path: String,
	},
}

pub enum ServerEvent {
	Connected {
		ch: mpsc::UnboundedSender<ClientMessage>,
	},
	ClientEvent {
		id: usize,
		event: ClientEvent,
	},
}

#[derive(Debug, Clone)]
pub enum Command {
	Render(Item),
	ReplaceRoot(Item),
	SetTitle(String),
	PushState(String),
	Navigate(String),
	Actions(Vec<ClientAction>),
}

pub type Clients = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Command>>>>;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn webrtc_join_deserializes_display_name_camel_or_snake() {
		let join_camel: WebRtcJoin = serde_json::from_str(
			r#"{"room":"channel:1","audio":true,"video":true,"displayName":"alice"}"#,
		)
		.unwrap();
		assert_eq!(join_camel.display_name.as_deref(), Some("alice"));

		let join_snake: WebRtcJoin = serde_json::from_str(
			r#"{"room":"channel:1","audio":true,"video":true,"display_name":"bob"}"#,
		)
		.unwrap();
		assert_eq!(join_snake.display_name.as_deref(), Some("bob"));
	}

	#[test]
	fn webrtc_signal_deserializes_target_client_id_camel_or_snake() {
		let signal_camel: WebRtcSignal =
			serde_json::from_str(r#"{"room":"channel:1","targetClientId":7,"payload":"x"}"#)
				.unwrap();
		assert_eq!(signal_camel.target_client_id, Some(7));

		let signal_snake: WebRtcSignal =
			serde_json::from_str(r#"{"room":"channel:1","target_client_id":9,"payload":"x"}"#)
				.unwrap();
		assert_eq!(signal_snake.target_client_id, Some(9));
	}

	#[test]
	fn webrtc_room_state_serializes_self_client_id_camel_case() {
		let action = ClientAction::WebRtcRoomState {
			room: "channel:1".to_string(),
			self_client_id: 5,
			peers: vec![3, 5],
			participants: vec![
				WebRtcParticipant {
					client_id: 3,
					display_name: "matti".to_string(),
				},
				WebRtcParticipant {
					client_id: 5,
					display_name: "jaska".to_string(),
				},
			],
		};
		let json = serde_json::to_value(&action).unwrap();
		assert_eq!(
			json.get("selfClientId").and_then(|value| value.as_u64()),
			Some(5)
		);
		assert!(json.get("self_client_id").is_none());
		assert_eq!(
			json.get("participants")
				.and_then(|value| value.as_array())
				.and_then(|value| value.first())
				.and_then(|value| value.get("clientId"))
				.and_then(|value| value.as_u64()),
			Some(3)
		);
	}

	#[test]
	fn webrtc_signal_serializes_from_client_id_camel_case() {
		let action = ClientAction::WebRtcSignal {
			room: "channel:1".to_string(),
			from_client_id: 3,
			payload: "hello".to_string(),
		};
		let json = serde_json::to_value(&action).unwrap();
		assert_eq!(
			json.get("fromClientId").and_then(|value| value.as_u64()),
			Some(3)
		);
		assert!(json.get("from_client_id").is_none());
	}

	#[test]
	fn web_push_subscription_changed_deserializes_from_camel_case_type() {
		let with_subscription: ClientEvent = serde_json::from_str(
			r#"{"type":"webPushSubscriptionChanged","subscription":{"endpoint":"https://push.example/sub"}}"#,
		)
		.unwrap();
		match with_subscription {
			ClientEvent::WebPushSubscriptionChanged(payload) => {
				assert_eq!(
					payload
						.subscription
						.as_ref()
						.and_then(|value| value.get("endpoint"))
						.and_then(|value| value.as_str()),
					Some("https://push.example/sub")
				);
			}
			_ => panic!("expected WebPushSubscriptionChanged"),
		}

		let without_subscription: ClientEvent =
			serde_json::from_str(r#"{"type":"webPushSubscriptionChanged","subscription":null}"#)
				.unwrap();
		match without_subscription {
			ClientEvent::WebPushSubscriptionChanged(payload) => {
				assert!(payload.subscription.is_none());
			}
			_ => panic!("expected WebPushSubscriptionChanged"),
		}
	}

	#[test]
	fn web_push_enable_serializes_camel_case_fields() {
		let action = ClientAction::WebPushEnable {
			service_worker_path: "/sw.js".to_string(),
			vapid_public_key: Some("public-key".to_string()),
		};
		let json = serde_json::to_value(&action).unwrap();
		assert_eq!(
			json.get("serviceWorkerPath")
				.and_then(|value| value.as_str()),
			Some("/sw.js")
		);
		assert_eq!(
			json.get("vapidPublicKey").and_then(|value| value.as_str()),
			Some("public-key")
		);
		assert!(json.get("service_worker_path").is_none());
		assert!(json.get("vapid_public_key").is_none());
	}
}
