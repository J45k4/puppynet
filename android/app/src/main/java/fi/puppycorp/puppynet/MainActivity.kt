package fi.puppycorp.puppynet

import android.content.Context
import android.os.Bundle
import android.util.Base64
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.weight
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.google.android.gms.code.scanner.GmsBarcodeScanning
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import org.json.JSONObject

class MainActivity : ComponentActivity() {
	override fun onCreate(savedInstanceState: Bundle?) {
		super.onCreate(savedInstanceState)
		setContent {
			MaterialTheme {
				PuppyNetApp(this)
			}
		}
	}
}

private fun pairingPeer(value: String): PeerEndpoint? = runCatching {
	val payload = JSONObject(value)
	check(payload.optInt("v") == 1)
	val nodeId = payload.getString("node_id")
	val endpoint = payload.getString("endpoint")
	val url = endpoint.toHttpUrlOrNull()
	check(nodeId.isNotBlank())
	check(url != null && (url.scheme == "ws" || url.scheme == "wss"))
	PeerEndpoint(nodeId, endpoint)
}.getOrNull()

private class PeerStore(context: Context) {
	private val preferences = context.getSharedPreferences("puppynet", Context.MODE_PRIVATE)
	val identitySeed: ByteArray = preferences.getString("identity_seed", null)?.let {
		Base64.decode(it, Base64.NO_WRAP)
	} ?: NativeSession.createIdentitySeed().also { seed ->
		preferences.edit().putString("identity_seed", Base64.encodeToString(seed, Base64.NO_WRAP)).apply()
	}

	fun peers(): List<PeerEndpoint> = preferences.getStringSet("peers", emptySet())
		.orEmpty()
		.mapNotNull { value ->
			val separator = value.indexOf('|')
			if (separator <= 0 || separator == value.lastIndex) null
			else PeerEndpoint(value.substring(0, separator), value.substring(separator + 1))
		}
		.sortedBy(PeerEndpoint::nodeId)

	fun save(peer: PeerEndpoint) {
		val peers = preferences.getStringSet("peers", emptySet()).orEmpty().toMutableSet()
		peers.removeAll { it.substringBefore('|') == peer.nodeId }
		peers.add("${peer.nodeId}|${peer.endpoint}")
		preferences.edit().putStringSet("peers", peers).apply()
	}
}

@androidx.compose.runtime.Composable
private fun PuppyNetApp(activity: ComponentActivity) {
	val store = remember { PeerStore(activity.applicationContext) }
	val scanner = remember(activity) { GmsBarcodeScanning.getClient(activity) }
	var peers by remember { mutableStateOf(store.peers()) }
	var nodeId by remember { mutableStateOf("") }
	var endpoint by remember { mutableStateOf("ws://") }
	var status by remember { mutableStateOf("Add a known PuppyNet node to connect.") }
	var connection by remember { mutableStateOf<PeerConnection?>(null) }

	Column(
		modifier = Modifier.fillMaxSize().padding(20.dp),
		verticalArrangement = Arrangement.spacedBy(12.dp),
	) {
		Text("PuppyNet", style = MaterialTheme.typography.headlineMedium)
		Text("This device: ${NativeSession.nodeId(store.identitySeed)}", style = MaterialTheme.typography.bodySmall)
		Card(modifier = Modifier.fillMaxWidth()) {
			Text(status, modifier = Modifier.padding(16.dp), style = MaterialTheme.typography.bodyMedium)
		}
		Button(
			onClick = {
				scanner.startScan()
					.addOnSuccessListener { barcode ->
						val peer = barcode.rawValue?.let(::pairingPeer)
						if (peer == null) {
							status = "This is not a PuppyNet pairing code."
						} else {
							store.save(peer)
							peers = store.peers()
							nodeId = peer.nodeId
							endpoint = peer.endpoint
							connection?.close()
							connection = PeerConnection(store.identitySeed, peer) { status = it }.also { it.connect() }
						}
					}
					.addOnFailureListener { error ->
						status = "QR scan failed: ${error.message ?: "unknown error"}"
					}
			},
			modifier = Modifier.fillMaxWidth(),
		) {
			Text("Scan PuppyNet pairing QR")
		}
		OutlinedTextField(
			value = nodeId,
			onValueChange = { nodeId = it.trim() },
			modifier = Modifier.fillMaxWidth(),
			label = { Text("Peer node ID") },
			singleLine = true,
		)
		OutlinedTextField(
			value = endpoint,
			onValueChange = { endpoint = it.trim() },
			modifier = Modifier.fillMaxWidth(),
			label = { Text("WebSocket endpoint") },
			placeholder = { Text("ws://192.168.1.20:8833") },
			singleLine = true,
		)
		Button(
			onClick = {
				if (nodeId.isEmpty() || endpoint == "ws://") {
					status = "Node ID and endpoint are required."
				} else {
					val peer = PeerEndpoint(nodeId, endpoint)
					store.save(peer)
					peers = store.peers()
					connection?.close()
					connection = PeerConnection(store.identitySeed, peer) { status = it }.also { it.connect() }
				}
			},
			modifier = Modifier.fillMaxWidth(),
		) {
			Text("Save and connect")
		}
		Text("Saved peers", style = MaterialTheme.typography.titleMedium)
		LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.weight(1f)) {
			items(peers, key = { it.nodeId }) { peer ->
				Card(modifier = Modifier.fillMaxWidth()) {
					Row(
						modifier = Modifier.fillMaxWidth().padding(12.dp),
						horizontalArrangement = Arrangement.spacedBy(8.dp),
					) {
						Column(modifier = Modifier.weight(1f)) {
							Text(peer.nodeId, style = MaterialTheme.typography.bodySmall)
							Spacer(Modifier.height(2.dp))
							Text(peer.endpoint, style = MaterialTheme.typography.bodyMedium)
						}
						Button(onClick = {
							connection?.close()
							connection = PeerConnection(store.identitySeed, peer) { status = it }.also { it.connect() }
						}) {
							Text("Connect")
						}
					}
				}
			}
		}
	}
}
