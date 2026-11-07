package com.nexora.mobile.net

import com.nexora.mobile.data.Identity
import com.nexora.mobile.data.Server
import okhttp3.ConnectionPool
import okhttp3.ConnectionSpec
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.TlsVersion
import java.io.ByteArrayInputStream
import java.net.Socket
import java.security.MessageDigest
import java.security.Principal
import java.security.PrivateKey
import java.security.cert.CertPathValidator
import java.security.cert.CertificateException
import java.security.cert.CertificateFactory
import java.security.cert.PKIXParameters
import java.security.cert.TrustAnchor
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager

/** Сертификат сервера не совпал с закреплённым отпечатком и не подписан нашим CA: возможна подмена сервера. */
class ServerIdentityException(msg: String) : CertificateException(msg)

fun sha256Hex(b: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it) }

private fun parseCert(pem: String): X509Certificate =
    CertificateFactory.getInstance("X.509").generateCertificate(ByteArrayInputStream(pem.toByteArray())) as X509Certificate

/**
 * Доверие только нашему серверу: системный список CA не используется вообще.
 * Принимается сертификат с закреплённым отпечатком либо цепочка, подписанная CA сервера (получен при привязке).
 * Имя хоста не проверяется: адрес может меняться (локальная сеть / VPN), личность сервера задаёт отпечаток или CA.
 */
class PinnedTrust(private val pin: String, caPem: String, private val host: String = "") : X509TrustManager {
    private val ca: X509Certificate? = caPem.takeIf { it.isNotBlank() }?.let { runCatching { parseCert(it) }.getOrNull() }

    override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) = throw CertificateException("клиентские сертификаты не принимаются")

    override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
        if (chain.isEmpty()) throw ServerIdentityException("пустая цепочка")
        if (sha256Hex(chain[0].encoded) == pin) return
        if (ca != null && isServerCert(chain[0])) {
            try {
                val cf = CertificateFactory.getInstance("X.509")
                val path = cf.generateCertPath(listOf(chain[0]))
                val params = PKIXParameters(setOf(TrustAnchor(ca, null))).apply { isRevocationEnabled = false }
                CertPathValidator.getInstance("PKIX").validate(path, params)
                return
            } catch (_: Exception) {
            }
        }
        throw ServerIdentityException("сертификат сервера не совпал с закреплённым")
    }

    /**
     * Сертификат, выданный нашим CA, считается сертификатом сервера, только если он предназначен для сервера
     * (EKU serverAuth, не CA) и содержит адрес, по которому мы подключаемся. Иначе сертификат устройства
     * (clientAuth), выданный тем же CA, мог бы выдать себя за сервер.
     */
    private fun isServerCert(c: X509Certificate): Boolean {
        if (c.basicConstraints != -1) return false
        if (c.extendedKeyUsage?.contains("1.3.6.1.5.5.7.3.1") != true) return false
        if (host.isEmpty()) return false
        val names = c.subjectAlternativeNames ?: return false
        return names.any { n -> (n[0] == 7 || n[0] == 2) && (n[1] as? String).equals(host, ignoreCase = true) }
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = ca?.let { arrayOf(it) } ?: emptyArray()
}

/** Клиентский сертификат устройства; закрытый ключ остаётся в Android Keystore и наружу не выходит. */
private class DeviceKeys(private val cert: X509Certificate, private val key: PrivateKey) : X509ExtendedKeyManager() {
    private fun d(m: String) { android.util.Log.w("nexora", "DeviceKeys: $m") }
    override fun chooseClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?): String { d("chooseClientAlias types=${keyType?.toList()} issuers=${issuers?.size}"); return "device" }
    override fun chooseEngineClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, engine: javax.net.ssl.SSLEngine?): String { d("chooseEngineClientAlias types=${keyType?.toList()} issuers=${issuers?.size}"); return "device" }
    override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?) = arrayOf("device")
    override fun getCertificateChain(alias: String?): Array<X509Certificate> { d("getCertificateChain subject=${cert.subjectX500Principal} issuer=${cert.issuerX500Principal} notBefore=${cert.notBefore} sigAlg=${cert.sigAlgName}"); return arrayOf(cert) }
    override fun getPrivateKey(alias: String?): PrivateKey { d("getPrivateKey alg=${key.algorithm} cls=${key.javaClass.simpleName}"); return key }
    override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? = null
    override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?): String? = null
}

object Tls {
    /** Откуда берётся закрытый ключ устройства. В приложении это Android Keystore; подменяется только в тестах. */
    var keyProvider: (String) -> PrivateKey? = { Identity.privateKey(it) }

    private val spec = ConnectionSpec.Builder(ConnectionSpec.RESTRICTED_TLS).tlsVersions(TlsVersion.TLS_1_3).build()

    private val cache = HashMap<String, OkHttpClient>()

    /** Один клиент на сервер и сертификат: не пересобираем TLS-контекст при каждом запросе. */
    @Synchronized
    fun cached(s: Server): OkHttpClient = cache.getOrPut("${s.id}|${s.fingerprint}|${s.certPem.hashCode()}") { client(s, withIdentity = true) }

    /** Клиент для сервера. Без withIdentity клиентский сертификат не предъявляется (нужно для привязки). */
    fun client(s: Server, withIdentity: Boolean, readTimeoutSec: Long = 15): OkHttpClient {
        val trust = PinnedTrust(s.fingerprint, s.caPem, s.host)
        val km = if (withIdentity && s.enrolled) {
            val key = keyProvider(s.id) ?: throw IllegalStateException("ключ устройства утерян")
            arrayOf(DeviceKeys(parseCert(s.certPem), key))
        } else null
        val ctx = SSLContext.getInstance("TLSv1.3").apply { init(km, arrayOf(trust), null) }
        return OkHttpClient.Builder()
            .sslSocketFactory(ctx.socketFactory, trust)
            .hostnameVerifier { _, _ -> true } // доверие задано отпечатком/CA, см. PinnedTrust
            .connectionSpecs(listOf(spec))
            .protocols(listOf(Protocol.HTTP_1_1))
            .connectionPool(ConnectionPool(0, 1, TimeUnit.SECONDS))
            .retryOnConnectionFailure(false) // повтор мог бы сжечь одноразовый код
            .connectTimeout(8, TimeUnit.SECONDS)
            .readTimeout(readTimeoutSec, TimeUnit.SECONDS)
            .writeTimeout(15, TimeUnit.SECONDS)
            .build()
    }
}
