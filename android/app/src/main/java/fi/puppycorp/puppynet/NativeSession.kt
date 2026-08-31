package fi.puppycorp.puppynet

object NativeSession {
	init {
		System.loadLibrary("puppynet_android")
	}

	external fun createIdentitySeed(): ByteArray
	external fun nodeId(seed: ByteArray): String
	external fun createInitiator(seed: ByteArray, expectedRemote: String): Long
	external fun nextHandshake(handle: Long): ByteArray?
	external fun readHandshake(handle: Long, message: ByteArray)
	external fun isComplete(handle: Long): Boolean
	external fun remoteNodeId(handle: Long): String
	external fun encrypt(handle: Long, payload: ByteArray): ByteArray
	external fun decrypt(handle: Long, payload: ByteArray): ByteArray
	external fun close(handle: Long)
}
