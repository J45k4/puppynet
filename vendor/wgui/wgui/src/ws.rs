#[cfg(feature = "hyper")]
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "hyper")]
use std::task::{Context, Poll};

use anyhow::Error;
use futures_util::{Sink, Stream};
#[cfg(feature = "hyper")]
use hyper_tungstenite::tungstenite::Message as TungsteniteMessage;
#[cfg(feature = "hyper")]
use hyper_util::rt::TokioIo;

static CLIENT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_client_id() -> usize {
	CLIENT_ID.fetch_add(1, Ordering::Relaxed) as usize
}

#[derive(Debug)]
pub enum WsMessage {
	Text(String),
	Binary(Vec<u8>),
	Ping(Vec<u8>),
	Pong(Vec<u8>),
	Close,
}

pub trait WsStream:
	Stream<Item = Result<WsMessage, Error>> + Sink<WsMessage, Error = Error> + Unpin + Send
{
}

impl<T> WsStream for T where
	T: Stream<Item = Result<WsMessage, Error>> + Sink<WsMessage, Error = Error> + Unpin + Send
{
}

#[cfg(feature = "hyper")]
pub struct TungsteniteWs<S> {
	inner: S,
}

#[cfg(feature = "hyper")]
impl<S> TungsteniteWs<S> {
	pub fn new(inner: S) -> Self {
		Self { inner }
	}
}

#[cfg(feature = "hyper")]
impl Stream
	for TungsteniteWs<hyper_tungstenite::WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>>
{
	type Item = Result<WsMessage, Error>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		match Pin::new(&mut self.inner).poll_next(cx) {
			Poll::Ready(Some(Ok(msg))) => Poll::Ready(Some(Ok(msg.into()))),
			Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err.into()))),
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Pending => Poll::Pending,
		}
	}
}

#[cfg(feature = "hyper")]
impl Sink<WsMessage>
	for TungsteniteWs<hyper_tungstenite::WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>>
{
	type Error = Error;

	fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		Pin::new(&mut self.inner)
			.poll_ready(cx)
			.map_err(Error::from)
	}

	fn start_send(mut self: Pin<&mut Self>, item: WsMessage) -> Result<(), Self::Error> {
		Pin::new(&mut self.inner)
			.start_send(item.try_into()?)
			.map_err(Error::from)
	}

	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		Pin::new(&mut self.inner)
			.poll_flush(cx)
			.map_err(Error::from)
	}

	fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		Pin::new(&mut self.inner)
			.poll_close(cx)
			.map_err(Error::from)
	}
}

#[cfg(feature = "hyper")]
impl From<TungsteniteMessage> for WsMessage {
	fn from(value: TungsteniteMessage) -> Self {
		match value {
			TungsteniteMessage::Text(msg) => WsMessage::Text(msg),
			TungsteniteMessage::Binary(msg) => WsMessage::Binary(msg),
			TungsteniteMessage::Ping(msg) => WsMessage::Ping(msg),
			TungsteniteMessage::Pong(msg) => WsMessage::Pong(msg),
			TungsteniteMessage::Close(_) => WsMessage::Close,
			TungsteniteMessage::Frame(_) => WsMessage::Close,
		}
	}
}

#[cfg(feature = "hyper")]
impl TryFrom<WsMessage> for TungsteniteMessage {
	type Error = Error;

	fn try_from(value: WsMessage) -> Result<Self, Self::Error> {
		let msg = match value {
			WsMessage::Text(msg) => TungsteniteMessage::Text(msg),
			WsMessage::Binary(msg) => TungsteniteMessage::Binary(msg),
			WsMessage::Ping(msg) => TungsteniteMessage::Ping(msg),
			WsMessage::Pong(msg) => TungsteniteMessage::Pong(msg),
			WsMessage::Close => TungsteniteMessage::Close(None),
		};

		Ok(msg)
	}
}
