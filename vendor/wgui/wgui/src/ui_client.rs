use super::{
	gui::Item,
	types::{ClientEvent, ClientMessage},
};
use crate::{
	diff::diff,
	types::{ClientAction, Clients, Command, Replace},
	ws::{WsMessage, WsStream},
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

fn event_kind_name(event: &ClientEvent) -> &'static str {
	match event {
		ClientEvent::Disconnected { .. } => "Disconnected",
		ClientEvent::Connected { .. } => "Connected",
		ClientEvent::PathChanged(_) => "PathChanged",
		ClientEvent::Input(_) => "Input",
		ClientEvent::OnClick(_) => "OnClick",
		ClientEvent::OnTextChanged(_) => "OnTextChanged",
		ClientEvent::OnSliderChange(_) => "OnSliderChange",
		ClientEvent::OnSelect(_) => "OnSelect",
		ClientEvent::OnCustom(_) => "OnCustom",
		ClientEvent::WebRtcJoin(_) => "WebRtcJoin",
		ClientEvent::WebRtcLeave(_) => "WebRtcLeave",
		ClientEvent::WebRtcSignal(_) => "WebRtcSignal",
		ClientEvent::WebPushSubscriptionChanged(_) => "WebPushSubscriptionChanged",
	}
}

pub struct UiWsWorker<S>
where
	S: WsStream,
{
	id: usize,
	ws: S,
	event_tx: mpsc::UnboundedSender<ClientMessage>,
	cmd_recv: mpsc::UnboundedReceiver<Command>,
	clients: Clients,
	last_root: Option<Item>,
}

impl<S> UiWsWorker<S>
where
	S: WsStream,
{
	pub async fn new(
		id: usize,
		ws: S,
		event_tx: mpsc::UnboundedSender<ClientMessage>,
		clients: Clients,
	) -> Self {
		log::info!("[{}] connection started", id);
		let (cmd_sender, cmd_recv) = mpsc::unbounded_channel();
		clients.write().await.insert(id, cmd_sender);
		event_tx
			.send(ClientMessage {
				client_id: id,
				event: ClientEvent::Connected { id },
			})
			.unwrap();
		Self {
			id,
			ws,
			cmd_recv: cmd_recv,
			event_tx,
			last_root: None,
			clients,
		}
	}

	pub async fn handle_websocket(&mut self, msg: WsMessage) -> anyhow::Result<()> {
		match msg {
			WsMessage::Text(msg) => {
				log::info!("received text frame ({} bytes)", msg.len());

				let msgs: Vec<ClientEvent> = serde_json::from_str(&msg)?;
				let kinds: Vec<&str> = msgs.iter().map(event_kind_name).collect();
				log::info!("received {} event(s): {:?}", msgs.len(), kinds);

				for msg in msgs {
					self.event_tx
						.send(ClientMessage {
							client_id: self.id,
							event: msg,
						})
						.unwrap();
				}
			}
			WsMessage::Binary(msg) => {
				println!("Received binary message: {:02X?}", msg);
				self.ws
					.send(WsMessage::Binary(b"Thank you, come again.".to_vec()))
					.await?;
			}
			WsMessage::Ping(msg) => {
				log::info!("Received ping message: {:02X?}", msg);
			}
			WsMessage::Pong(msg) => {
				log::info!("Received pong message: {:02X?}", msg);
			}
			WsMessage::Close => {
				println!("Received close message");
			}
		};

		Ok(())
	}

	async fn handle_command(&mut self, cmd: Command) -> anyhow::Result<()> {
		log::debug!("handling command: {:?}", cmd);
		match cmd {
			Command::Render(root) => {
				let changes = match &self.last_root {
					Some(last_root) => {
						let changes = diff(&last_root, &root);
						changes
					}
					None => vec![ClientAction::Replace(Replace {
						path: vec![],
						item: root.clone(),
					})],
				};
				if changes.len() == 0 {
					return Ok(());
				}
				self.last_root = Some(root);
				log::debug!("sending changes: {:?}", changes);
				let str = serde_json::to_string(&changes).unwrap();
				self.ws.send(WsMessage::Text(str)).await?;
			}
			Command::ReplaceRoot(root) => {
				self.last_root = Some(root.clone());
				let changes = vec![ClientAction::Replace(Replace {
					path: vec![],
					item: root,
				})];
				let str = serde_json::to_string(&changes).unwrap();
				self.ws.send(WsMessage::Text(str)).await?;
			}
			Command::SetTitle(title) => {
				let changes = vec![ClientAction::SetTitle { title }];
				let str = serde_json::to_string(&changes).unwrap();
				self.ws.send(WsMessage::Text(str)).await?;
			}
			Command::PushState(url) => {
				let changes = vec![ClientAction::PushState(crate::types::PushState { url })];
				let str = serde_json::to_string(&changes).unwrap();
				self.ws.send(WsMessage::Text(str)).await?;
			}
			Command::Navigate(url) => {
				let changes = vec![ClientAction::Navigate(crate::types::Navigate { url })];
				let str = serde_json::to_string(&changes).unwrap();
				self.ws.send(WsMessage::Text(str)).await?;
			}
			Command::Actions(actions) => {
				if actions.is_empty() {
					return Ok(());
				}
				let str = serde_json::to_string(&actions).unwrap();
				self.ws.send(WsMessage::Text(str)).await?;
			}
		};

		Ok(())
	}

	pub async fn run(mut self) {
		loop {
			tokio::select! {
				msg = self.ws.next() => {
					match msg {
						Some(msg) => match msg {
							Ok(msg) => {
								match self.handle_websocket(msg).await {
									Ok(_) => {},
									Err(err) => {
										log::error!("Error handling websocket message: {}", err);
									},
								}
							},
							Err(err) => {
								log::error!("Error receiving websocket message: {}", err);

								break;
							},
						},
						None => {
							log::error!("Websocket closed");

							break;
						},
					}
				}
				cmd = self.cmd_recv.recv() => {
					match cmd {
						Some(cmd) => {
							match self.handle_command(cmd).await {
								Ok(_) => {},
								Err(err) => {
									log::error!("Error handling command: {}", err);
								}
							}
						}
						None => {
							log::error!("Command channel closed");

							break;
						}
					}
				}
			};
		}

		log::info!("[{}] connection closed", self.id);
		self.clients.write().await.remove(&self.id);
		self.event_tx
			.send(ClientMessage {
				client_id: self.id,
				event: ClientEvent::Disconnected { id: self.id },
			})
			.unwrap();
	}
}
