package com.nexora.mobile.data

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** Сохранённый чат: только текст реплик (действия агента и подтверждения не хранятся). */
class StoredChat(val id: String, var title: String, var updated: Long, val msgs: MutableList<Pair<Boolean, String>> = mutableListOf())

/** Чаты лежат в закрытой папке приложения (резервное копирование выключено). Файл пишется целиком и атомарно. */
class ChatStore(ctx: Context) {
    private val file = File(ctx.filesDir, "chats.json")

    fun load(): MutableList<StoredChat> = runCatching {
        val a = JSONArray(file.readText())
        MutableList(a.length()) { i ->
            val o = a.getJSONObject(i)
            val m = o.getJSONArray("m")
            StoredChat(o.getString("id"), o.optString("t"), o.optLong("u"), MutableList(m.length()) { j -> m.getJSONObject(j).let { it.getBoolean("u") to it.getString("x") } })
        }
    }.getOrDefault(mutableListOf())

    fun save(list: List<StoredChat>) {
        val a = JSONArray()
        list.forEach { c ->
            a.put(JSONObject().put("id", c.id).put("t", c.title).put("u", c.updated).put("m", JSONArray(c.msgs.map { JSONObject().put("u", it.first).put("x", it.second) })))
        }
        val tmp = File(file.path + ".tmp")
        tmp.writeText(a.toString())
        tmp.renameTo(file)
    }
}
