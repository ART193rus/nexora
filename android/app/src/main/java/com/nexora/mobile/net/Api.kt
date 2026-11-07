package com.nexora.mobile.net

import com.nexora.mobile.data.Server
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.withContext
import okhttp3.Call
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import org.json.JSONArray
import org.json.JSONObject
import java.io.EOFException
import java.io.IOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.UnknownHostException

/** Что пошло не так, в терминах, понятных пользователю. */
sealed class NetError(msg: String) : IOException(msg) {
    /** Сервер не отвечает: нет сети, выключен, чужой адрес. */
    class Unreachable(why: String) : NetError(why)

    /** Соединение установилось, но сервер молча закрыл его: отозванное устройство, закрытое окно привязки или чужой сертификат. */
    class Rejected(val detail: String = "") : NetError("Сервер закрыл соединение без ответа")

    /** Сертификат сервера не тот, что был закреплён при привязке. */
    class IdentityChanged : NetError("Сертификат сервера изменился: возможна подмена")

    class Http(val code: Int, val body: String) : NetError("Ответ сервера: $code")
}

/** Короткая цепочка причин для показа и журнала: «класс: сообщение <- класс: сообщение». */
fun chain(e: Throwable): String {
    val out = mutableListOf<String>()
    var t: Throwable? = e
    while (t != null && out.size < 4) { out.add(t.javaClass.simpleName + (t.message?.let { ": " + it.take(120) } ?: "")); t = t.cause }
    return out.joinToString(" <- ")
}

fun classify(e: Throwable): NetError {
    var t: Throwable? = e
    while (t != null) {
        if (t is NetError) return t
        if (t is ServerIdentityException) return NetError.IdentityChanged()
        t = t.cause
    }
    return when (e) {
        is UnknownHostException -> NetError.Unreachable("Адрес сервера не найден")
        is ConnectException, is NoRouteToHostException -> NetError.Unreachable("Сервер недоступен")
        is SocketTimeoutException -> NetError.Unreachable("Сервер не отвечает")
        is javax.net.ssl.SSLException, is EOFException, is java.net.SocketException -> NetError.Rejected(chain(e)).also { android.util.Log.w("nexora", "отказ соединения: ${chain(e)}", e) }
        else -> NetError.Unreachable(e.message ?: "Ошибка сети")
    }
}

data class EnrollResult(val certPem: String, val caPem: String, val serial: String)

sealed interface ChatEvent {
    data class Delta(val text: String) : ChatEvent
    data class Reasoning(val text: String) : ChatEvent
    data object Done : ChatEvent
}

data class ServerStatus(val ready: Boolean, val busy: Boolean, val model: String?, val tools: Boolean, val active: Int, val trusted: Boolean = false, val trustPending: Boolean = false)

data class AppModels(val current: String?, val ready: Boolean, val models: List<Pair<String, String>>)

data class Pending(val id: Long, val kind: String, val title: String, val detail: String, val dangerous: Boolean)

data class TaskInfo(
    val id: Long, val title: String, val status: String, val created: Long, val started: Long, val finished: Long,
    val steps: Int, val hasResult: Boolean, val error: String, val pending: Pending?,
) {
    val active get() = status == "queued" || status == "running" || status == "waiting"
}

data class TaskEvent(val seq: Long, val ts: Long, val kind: String, val data: JSONObject)

data class TaskView(val info: TaskInfo, val events: List<TaskEvent>, val lastSeq: Long, val result: String?)

internal fun parseTask(o: JSONObject): TaskInfo = TaskInfo(
    o.getLong("id"), o.optString("title"), o.getString("status"), o.optLong("created"), o.optLong("started"), o.optLong("finished"),
    o.optInt("steps"), o.optBoolean("has_result"), o.optString("error"),
    o.optJSONObject("pending")?.let { Pending(it.optLong("id"), it.optString("kind"), it.optString("title"), it.optString("detail"), it.optBoolean("dangerous", true)) },
)

private val JSON = "application/json; charset=utf-8".toMediaType()

fun url(s: Server, path: String): HttpUrl = HttpUrl.Builder().scheme("https").host(s.host).port(s.port).encodedPath(path).build()

class Api(private val s: Server, private val http: OkHttpClient = Tls.cached(s)) {

    private fun get(path: String) = Request.Builder().url(url(s, path)).header("Accept", "application/json").get().build()

    private fun <T> exec(req: Request, parse: (Response) -> T): T = try {
        http.newCall(req).execute().use { r ->
            if (!r.isSuccessful) throw NetError.Http(r.code, r.body?.string().orEmpty().take(500))
            parse(r)
        }
    } catch (e: Throwable) {
        throw classify(e)
    }

    suspend fun ping(): String = withContext(Dispatchers.IO) {
        exec(get("/ping")) { JSONObject(it.body!!.string()).optString("device") }
    }

    suspend fun models(): List<String> = withContext(Dispatchers.IO) {
        exec(get("/v1/models")) {
            val a = JSONObject(it.body!!.string()).optJSONArray("data") ?: JSONArray()
            List(a.length()) { i -> a.getJSONObject(i).getString("id") }
        }
    }

    private val longHttp: OkHttpClient by lazy { http.newBuilder().readTimeout(40, java.util.concurrent.TimeUnit.SECONDS).build() }

    private fun <T> execWith(client: OkHttpClient, req: Request, parse: (Response) -> T): T = try {
        client.newCall(req).execute().use { r ->
            if (!r.isSuccessful) throw NetError.Http(r.code, r.body?.string().orEmpty().take(500))
            parse(r)
        }
    } catch (e: Throwable) {
        throw classify(e)
    }

    private fun post(path: String, body: JSONObject = JSONObject()) =
        Request.Builder().url(url(s, path)).post(body.toString().toRequestBody(JSON)).build()

    suspend fun status(): ServerStatus = withContext(Dispatchers.IO) {
        exec(get("/v1/status")) {
            val o = JSONObject(it.body!!.string())
            ServerStatus(o.optBoolean("ready"), o.optBoolean("busy"), o.optString("model").takeIf { m -> m.isNotEmpty() && m != "null" }, o.optBoolean("tools"), o.optInt("active_tasks"), o.optBoolean("trusted"), o.optBoolean("trust_pending"))
        }
    }

    suspend fun tasks(): List<TaskInfo> = withContext(Dispatchers.IO) {
        exec(get("/v1/tasks")) {
            val a = JSONObject(it.body!!.string()).optJSONArray("tasks") ?: JSONArray()
            List(a.length()) { i -> parseTask(a.getJSONObject(i)) }
        }
    }

    suspend fun createTask(prompt: String, title: String = "", history: List<Pair<String, String>> = emptyList(), fast: Boolean = false): Long = withContext(Dispatchers.IO) {
        val b = JSONObject().put("prompt", prompt).put("title", title).put("mode", if (fast) "fast" else "agent")
            .put("history", JSONArray(history.map { JSONObject().put("role", it.first).put("content", it.second) }))
        exec(post("/v1/tasks", b)) { JSONObject(it.body!!.string()).getLong("id") }
    }

    /** События задачи после [after]. Если новых нет, сервер держит запрос до [waitSec] секунд (длинный опрос). */
    suspend fun poll(id: Long, after: Long, waitSec: Int): TaskView = withContext(Dispatchers.IO) {
        execWith(longHttp, Request.Builder().url(url(s, "/v1/tasks/$id").newBuilder().addQueryParameter("after", after.toString()).addQueryParameter("wait", waitSec.toString()).build()).get().build()) {
            val o = JSONObject(it.body!!.string())
            val ev = o.optJSONArray("events") ?: JSONArray()
            TaskView(
                parseTask(o),
                List(ev.length()) { i -> ev.getJSONObject(i).let { e -> TaskEvent(e.getLong("seq"), e.optLong("ts"), e.getString("kind"), e.optJSONObject("data") ?: JSONObject()) } },
                o.optLong("last_seq", after),
                if (o.isNull("result")) null else o.optString("result"),
            )
        }
    }

    /** Модели приложения на ПК и выбранная сейчас. */
    suspend fun appModels(): AppModels = withContext(Dispatchers.IO) {
        exec(get("/v1/app/models")) {
            val o = JSONObject(it.body!!.string())
            val a = o.optJSONArray("models") ?: JSONArray()
            AppModels(o.optString("current").takeIf { c -> c.isNotEmpty() && c != "null" }, o.optBoolean("ready"),
                List(a.length()) { i -> a.getJSONObject(i).let { m -> m.getString("id") to m.optString("label", m.getString("id")) } })
        }
    }

    /** Выбор модели на ПК. Перезапускает движок, поэтому занимает до пары минут. */
    suspend fun selectModel(id: String) = withContext(Dispatchers.IO) { exec(post("/v1/app/model", JSONObject().put("id", id))) { } }

    /** Запрос «режима действий». Одобряет владелец в программе на ПК, телефон только ждёт решения. */
    suspend fun requestTrust() = withContext(Dispatchers.IO) { exec(post("/v1/trust/request")) { } }

    suspend fun revokeTrust() = withContext(Dispatchers.IO) { exec(post("/v1/trust/revoke")) { } }

    suspend fun cancel(id: Long) = withContext(Dispatchers.IO) { exec(post("/v1/tasks/$id/cancel")) { } }

    suspend fun retry(id: Long): Long = withContext(Dispatchers.IO) { exec(post("/v1/tasks/$id/retry")) { JSONObject(it.body!!.string()).getLong("id") } }

    /** Ответ на запрос подтверждения. Для опасных действий нужен свежий код Authenticator. */
    suspend fun approve(task: Long, approval: Long, ok: Boolean, code: String = "") = withContext(Dispatchers.IO) {
        exec(post("/v1/approvals", JSONObject().put("task", task).put("approval", approval).put("ok", ok).put("code", code))) { }
    }

    /** Выход: устройство отзывает собственный сертификат на сервере. */
    suspend fun revokeSelf() = withContext(Dispatchers.IO) { exec(post("/v1/devices/self/revoke")) { } }

    /** Потоковый чат (SSE) через прокси сервера. Отмена сбора потока закрывает соединение, и генерация останавливается. */
    fun chat(messages: List<Pair<String, String>>): Flow<ChatEvent> = callbackFlow {
        val body = JSONObject().put("stream", true).put(
            "messages", JSONArray(messages.map { JSONObject().put("role", it.first).put("content", it.second) }),
        ).toString()
        val client = Tls.client(s, withIdentity = true, readTimeoutSec = 600)
        val call: Call = client.newCall(Request.Builder().url(url(s, "/v1/chat/completions")).post(body.toRequestBody(JSON)).build())
        val worker = Thread {
            try {
                call.execute().use { r ->
                    if (!r.isSuccessful) throw NetError.Http(r.code, r.body?.string().orEmpty().take(300))
                    val src = r.body!!.source()
                    while (true) {
                        val line = src.readUtf8Line() ?: break
                        if (!line.startsWith("data:")) continue
                        val d = line.removePrefix("data:").trim()
                        if (d == "[DONE]") break
                        val delta = runCatching { JSONObject(d).getJSONArray("choices").getJSONObject(0).getJSONObject("delta") }.getOrNull() ?: continue
                        delta.optString("reasoning_content").takeIf { it.isNotEmpty() }?.let { trySend(ChatEvent.Reasoning(it)) }
                        delta.optString("content").takeIf { it.isNotEmpty() }?.let { trySend(ChatEvent.Delta(it)) }
                    }
                }
                trySend(ChatEvent.Done)
                close()
            } catch (e: Throwable) {
                if (call.isCanceled()) close() else close(classify(e))
            }
        }
        worker.start()
        awaitClose { call.cancel() }
    }.flowOn(Dispatchers.IO)

    companion object {
        /** Привязка нового устройства. Клиентский сертификат не предъявляется: сервер принимает такой запрос только при открытом окне привязки. */
        suspend fun enroll(s: Server, code: String, deviceName: String, csrPem: String): EnrollResult = withContext(Dispatchers.IO) {
            val http = Tls.client(s, withIdentity = false)
            val body = JSONObject().put("code", code).put("name", deviceName).put("csr", csrPem).toString()
            val req = Request.Builder().url(url(s, "/enroll")).post(body.toRequestBody(JSON)).build()
            try {
                http.newCall(req).execute().use { r ->
                    val text = r.body?.string().orEmpty()
                    if (!r.isSuccessful) throw NetError.Http(r.code, text.take(300))
                    val o = JSONObject(text)
                    EnrollResult(o.getString("cert"), o.getString("ca"), o.optString("serial"))
                }
            } catch (e: Throwable) {
                throw classify(e)
            }
        }
    }
}
