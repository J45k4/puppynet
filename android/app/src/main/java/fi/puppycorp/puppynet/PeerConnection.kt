package fi.puppycorp.puppynet

import android.os.Handler
import android.os.Looper
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okio.ByteString
import java.util.concurrent.atomic.AtomicBoolean

data class PeerEndpoint(
	val nodeId: String,
	val endpoint: String,
)

class PeerConnection(
	private val identitySeed: ByteArray,
	private val peer: PeerEndpoint,
	private val onStatus: (String) -> Unit,
) {
	private val mainHandler = Handler(Looper.getMainLooper())
	private val client = OkHttpClient()
	private val closed = AtomicBoolean()
	private var handle = 0L
	private var socket: WebSocket? = null

	fun connect() {
		val url = peer.endpoint.toHttpUrlOrNull()
		if (url == null || (url.scheme != "ws" && url.scheme != "wss")) {
			status("Endpoint must start with ws:// or wss://")
			return
		}
		val peerUrl = if (url.encodedPath == "/") {
			url.newBuilder().addPathSegments("peer/v1").build()
		} else {
			url
		}
		status("Connecting to ${peerUrl.host}")
		socket = client.newWebSocket(
			Request.Builder().url(peerUrl).build(),
			object : WebSocketListener() {
				override fun onOpen(webSocket: WebSocket, response: Response) {
					try {
						handle = NativeSession.createInitiator(identitySeed, peer.nodeId)
						sendHandshake(webSocket)
						status("Verifying ${peer.nodeId}")
					} catch (error: Exception) {
						fail(webSocket, error.message ?: "could not start secure session")
					}
				}

				override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
					try {
						if (!NativeSession.isComplete(handle)) {
							NativeSession.readHandshake(handle, bytes.toByteArray())
							sendHandshake(webSocket)
							if (NativeSession.isComplete(handle)) {
								val remote = NativeSession.remoteNodeId(handle)
								status("Connected and verified: $remote")
								val ping = "{\"Ping\":{\"id\":1}}".encodeToByteArray()
								webSocket.send(NativeSession.encrypt(handle, ping).toByteString())
							}
						} else {
							val message = NativeSession.decrypt(handle, bytes.toByteArray())
							if (message.isNotEmpty()) {
								status("Connected: ${message.decodeToString()}")
							}
						}
					} catch (error: Exception) {
						fail(webSocket, error.message ?: "secure session failed")
					}
				}

				override fun onMessage(webSocket: WebSocket, text: String) {
					fail(webSocket, "peer sent an unexpected text WebSocket message")
				}

				override fun onFailure(webSocket: WebSocket, throwable: Throwable, response: Response?) {
					finish("Connection failed: ${throwable.message ?: "unknown error"}")
				}

				override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
					finish("Connection closed")
				}
			},
		)
	}

	fun close() {
		socket?.close(1000, "user disconnected")
		finish("Disconnected")
	}

	private fun sendHandshake(webSocket: WebSocket) {
		NativeSession.nextHandshake(handle)?.let { message ->
			webSocket.send(message.toByteString())
		}
	}

	private fun fail(webSocket: WebSocket, message: String) {
		webSocket.close(1008, message)
		finish("Connection failed: $message")
	}

	private fun finish(message: String) {
		if (closed.compareAndSet(false, true)) {
			if (handle != 0L) {
				NativeSession.close(handle)
				handle = 0L
			}
			status(message)
		}
	}

	private fun status(message: String) {
		mainHandler.post { onStatus(message) }
	}
}
