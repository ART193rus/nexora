package com.nexora.mobile.data

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.UUID

/**
 * Сохранённый сервер. Здесь только публичные данные: адрес, отпечаток, сертификат CA и сертификат устройства.
 * Закрытого ключа и секрета Authenticator тут нет: ключ в Android Keystore, секрет остаётся на сервере.
 */
data class Server(
    val id: String,
    val name: String,
    val host: String,
    val port: Int,
    val fingerprint: String,   // SHA-256 сертификата сервера, hex
    val caPem: String = "",
    val certPem: String = "",
    val serial: String = "",
) {
    val enrolled get() = certPem.isNotEmpty()
    val label get() = "$host:$port"

    fun toJson() = JSONObject().put("id", id).put("name", name).put("host", host).put("port", port)
        .put("fp", fingerprint).put("ca", caPem).put("cert", certPem).put("serial", serial)

    companion object {
        fun fromJson(o: JSONObject) = Server(
            o.getString("id"), o.optString("name"), o.getString("host"), o.getInt("port"),
            o.getString("fp"), o.optString("ca"), o.optString("cert"), o.optString("serial"),
        )

        fun newId() = UUID.randomUUID().toString().take(8)
    }
}

/** Разбор строки подключения nexora://адрес:порт#отпечаток. Отпечаток обязателен: без него сервер не проверить. */
data class Link(val host: String, val port: Int, val fingerprint: String) {
    companion object {
        fun parse(raw: String): Link? {
            val s = raw.trim()
            if (!s.startsWith("nexora://", ignoreCase = true)) return null
            val rest = s.substring(9)
            val (hp, fp) = rest.split('#', limit = 2).let { it[0] to it.getOrElse(1) { "" } }
            val fpn = fp.lowercase().filter { it.isLetterOrDigit() }
            if (fpn.length != 64 || !fpn.all { it in '0'..'9' || it in 'a'..'f' }) return null
            val hostPart = hp.trimEnd('/')
            val host: String
            val port: Int
            if (hostPart.startsWith("[")) { // IPv6
                val end = hostPart.indexOf(']')
                if (end < 0) return null
                host = hostPart.substring(1, end)
                port = hostPart.substring(end + 1).removePrefix(":").toIntOrNull() ?: return null
            } else {
                val i = hostPart.lastIndexOf(':')
                if (i < 0) return null
                host = hostPart.substring(0, i)
                port = hostPart.substring(i + 1).toIntOrNull() ?: return null
            }
            if (host.isBlank() || port !in 1..65535) return null
            if (!host.all { it.isLetterOrDigit() || it in ".-:" }) return null
            return Link(host, port, fpn)
        }
    }
}

class ServerStore(ctx: Context) {
    private val file = File(ctx.filesDir, "servers.json")
    private var list: MutableList<Server> = load()
    var activeId: String? = list.firstOrNull { it.enrolled }?.id
        private set

    private fun load(): MutableList<Server> = runCatching {
        val o = JSONObject(file.readText())
        activeId = o.optString("active").ifEmpty { null }
        val a = o.getJSONArray("servers")
        MutableList(a.length()) { Server.fromJson(a.getJSONObject(it)) }
    }.getOrDefault(mutableListOf())

    private fun save() {
        val tmp = File(file.path + ".tmp")
        tmp.writeText(JSONObject().put("active", activeId ?: "").put("servers", JSONArray(list.map { it.toJson() })).toString())
        tmp.renameTo(file)
    }

    fun all(): List<Server> = list.toList()
    fun active(): Server? = list.firstOrNull { it.id == activeId }

    fun upsert(s: Server, makeActive: Boolean = true) {
        val i = list.indexOfFirst { it.id == s.id }
        if (i >= 0) list[i] = s else list.add(s)
        if (makeActive) activeId = s.id
        save()
    }

    fun setActive(id: String) { activeId = id; save() }

    fun remove(id: String) {
        list.removeAll { it.id == id }
        Identity.delete(id)
        if (activeId == id) activeId = list.firstOrNull()?.id
        save()
    }
}
