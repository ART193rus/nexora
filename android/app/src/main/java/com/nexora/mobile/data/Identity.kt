package com.nexora.mobile.data

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.io.ByteArrayOutputStream
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.PublicKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * Ключ устройства: ECDSA P-256 в Android Keystore, не выгружаемый (StrongBox, если есть).
 * На сервер уходит только запрос на сертификат (CSR), подписанный этим ключом на самом телефоне.
 * Для каждого сервера свой ключ: alias = "nexora-" + id сервера.
 */
object Identity {
    private const val PROVIDER = "AndroidKeyStore"

    private fun ks(): KeyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }

    fun alias(serverId: String) = "nexora-$serverId"

    fun exists(serverId: String) = ks().containsAlias(alias(serverId))

    fun privateKey(serverId: String): PrivateKey? = ks().getKey(alias(serverId), null) as? PrivateKey

    fun delete(serverId: String) {
        runCatching { ks().deleteEntry(alias(serverId)) }
    }

    /** Создать новую пару ключей (старая с тем же именем заменяется) и вернуть открытый ключ. */
    fun create(serverId: String): PublicKey {
        delete(serverId)
        fun gen(strongBox: Boolean) = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, PROVIDER).apply {
            val b = KeyGenParameterSpec.Builder(alias(serverId), KeyProperties.PURPOSE_SIGN)
                .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                .setDigests(KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_NONE) // NONE нужен TLS-стеку Android для подписи рукопожатия
                .setUserAuthenticationRequired(true) // ключ работает только после биометрии / PIN экрана, см. Gate
            if (Build.VERSION.SDK_INT >= 30) {
                b.setUserAuthenticationParameters(Gate.WINDOW_SEC, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
            } else {
                @Suppress("DEPRECATION") b.setUserAuthenticationValidityDurationSeconds(Gate.WINDOW_SEC)
            }
            if (strongBox) b.setIsStrongBoxBacked(true)
            initialize(b.build())
        }.generateKeyPair()
        val kp = try {
            gen(true)
        } catch (_: Exception) {
            gen(false)
        }
        return kp.public
    }

    /** PKCS#10 CSR в PEM, подписанный ключом из Keystore. */
    fun csrPem(serverId: String, commonName: String, pub: PublicKey): String =
        buildCsr(commonName, pub, privateKey(serverId) ?: error("ключ не найден"))

    /** Сборка CSR из готовой пары ключей (отдельно, чтобы проверять без Keystore). */
    fun buildCsr(commonName: String, pub: PublicKey, key: PrivateKey): String {
        val info = Der.seq(
            Der.int0(),
            Der.seq(Der.set(Der.seq(Der.oid(byteArrayOf(0x55, 0x04, 0x03)), Der.utf8(commonName)))),
            pub.encoded, // SubjectPublicKeyInfo уже в DER
            byteArrayOf(0xA0.toByte(), 0x00), // attributes [0] пустое множество
        )
        val sig = Signature.getInstance("SHA256withECDSA").run {
            initSign(key); update(info); sign()
        }
        val sigAlg = Der.seq(Der.oid(byteArrayOf(0x2A, 0x86.toByte(), 0x48, 0xCE.toByte(), 0x3D, 0x04, 0x03, 0x02)))
        val csr = Der.seq(info, sigAlg, Der.bitString(sig))
        val b64 = java.util.Base64.getEncoder().encodeToString(csr).chunked(64).joinToString("\n")
        return "-----BEGIN CERTIFICATE REQUEST-----\n$b64\n-----END CERTIFICATE REQUEST-----\n"
    }
}

/** Минимальный кодировщик DER, достаточно для CSR. */
object Der {
    private fun len(n: Int): ByteArray = when {
        n < 0x80 -> byteArrayOf(n.toByte())
        n < 0x100 -> byteArrayOf(0x81.toByte(), n.toByte())
        else -> byteArrayOf(0x82.toByte(), (n shr 8).toByte(), n.toByte())
    }

    private fun tlv(tag: Int, body: ByteArray): ByteArray {
        val o = ByteArrayOutputStream()
        o.write(tag); o.write(len(body.size)); o.write(body)
        return o.toByteArray()
    }

    private fun cat(vararg p: ByteArray): ByteArray = ByteArrayOutputStream().also { o -> p.forEach { o.write(it) } }.toByteArray()

    fun seq(vararg p: ByteArray) = tlv(0x30, cat(*p))
    fun set(vararg p: ByteArray) = tlv(0x31, cat(*p))
    fun oid(b: ByteArray) = tlv(0x06, b)
    fun utf8(s: String) = tlv(0x0C, s.toByteArray(Charsets.UTF_8))
    fun int0() = tlv(0x02, byteArrayOf(0))
    fun bitString(b: ByteArray) = tlv(0x03, cat(byteArrayOf(0), b))
}
