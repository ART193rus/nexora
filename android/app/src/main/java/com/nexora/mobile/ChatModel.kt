package com.nexora.mobile

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.nexora.mobile.data.Server
import com.nexora.mobile.data.ChatStore
import com.nexora.mobile.data.StoredChat
import com.nexora.mobile.net.Api
import com.nexora.mobile.net.AppModels
import com.nexora.mobile.net.NetError
import com.nexora.mobile.net.Pending
import com.nexora.mobile.net.ServerStatus
import com.nexora.mobile.net.TaskEvent
import com.nexora.mobile.net.follow
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/** Действие агента (вызов инструмента): название, статус и вывод. */
class Action(val id: String, title: String, status: String, output: String = "") {
    var title by mutableStateOf(title)
    var status by mutableStateOf(status) // running | ok | error | denied
    var output by mutableStateOf(output)
}

class Msg(val fromUser: Boolean, text: String = "") {
    var text by mutableStateOf(text)
    var thinking by mutableStateOf("")
    var error by mutableStateOf<String?>(null)
    var streaming by mutableStateOf(false)
    val actions = mutableStateListOf<Action>()
    var approval by mutableStateOf<Pending?>(null)
    var approvalError by mutableStateOf<String?>(null)
    var taskId by mutableStateOf<Long?>(null)
}

/** Разбор событий задачи в сообщение чата. Общая логика для чата и для экрана задачи. */
fun applyEvents(m: Msg, events: List<TaskEvent>) {
    for (e in events) when (e.kind) {
        "token" -> {
            val t = e.data.optString("text")
            if (e.data.optString("kind") == "reasoning") m.thinking += t else m.text += t
        }
        "tool" -> {
            val id = e.data.optString("id")
            val st = e.data.optString("status")
            val title = e.data.optString("title")
            val a = m.actions.firstOrNull { it.id == id }
            if (a == null) m.actions.add(Action(id, title, st, e.data.optString("output"))) else { a.title = title; a.status = st; a.output = e.data.optString("output") }
        }
        "approval" -> {
            m.approvalError = null
            m.approval = Pending(e.data.optLong("id"), e.data.optString("kind"), e.data.optString("title"), e.data.optString("detail"), e.data.optBoolean("dangerous", true))
        }
        "approval_resolved" -> m.approval = null
    }
}

/** Чат идёт через агента: каждое сообщение это задача на сервере, события приходят потоком. Живёт на уровне приложения. */
data class ChatMeta(val id: String, val title: String, val updated: Long)

class ChatModel(private val store: ChatStore, private val prefs: Prefs, private val onTaskStarted: () -> Unit = {}) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    val messages = mutableStateListOf<Msg>()
    private val all: MutableList<StoredChat> = store.load()
    var chats by mutableStateOf<List<ChatMeta>>(emptyList())
        private set
    var currentId by mutableStateOf("")
        private set
    var models by mutableStateOf<AppModels?>(null)
        private set
    var switching by mutableStateOf(false)
        private set
    var notice by mutableStateOf<String?>(null)
    /** Быстрый чат: короткая подсказка без инструментов (на слабом ЦП ответ идёт в разы быстрее). */
    var fast by mutableStateOf(prefs.fastChat)
        private set

    fun useFast(v: Boolean) { fast = v; prefs.fastChat = v }

    private fun refreshList() { chats = all.sortedByDescending { it.updated }.map { ChatMeta(it.id, it.title, it.updated) } }

    private fun openInternal(id: String) {
        val c = all.first { it.id == id }
        currentId = id
        messages.clear()
        c.msgs.forEach { (u, t) -> messages.add(Msg(u, t)) }
        lastOutcome = null
        refreshList()
    }

    /** Записать текущий чат. Название берётся из первой реплики. */
    private fun persist() {
        val c = all.firstOrNull { it.id == currentId } ?: return
        c.msgs.clear()
        messages.filter { it.error == null && it.text.isNotEmpty() }.forEach { c.msgs.add(it.fromUser to it.text) }
        if (c.msgs.isNotEmpty()) { c.updated = System.currentTimeMillis(); c.msgs.firstOrNull { it.first }?.let { c.title = it.second.take(40) } }
        runCatching { store.save(all) }
        refreshList()
    }

    fun newChat() {
        if (busy) return
        persist()
        val cur = all.firstOrNull { it.id == currentId }
        if (cur != null && cur.msgs.isEmpty()) return
        val c = StoredChat(java.util.UUID.randomUUID().toString().take(8), "Новый чат", System.currentTimeMillis())
        all.add(c)
        openInternal(c.id)
    }

    fun openChat(id: String) { if (busy || id == currentId) return; persist(); openInternal(id) }

    fun deleteChat(id: String) {
        if (busy) return
        all.removeAll { it.id == id }
        if (all.isEmpty()) all.add(StoredChat(java.util.UUID.randomUUID().toString().take(8), "Новый чат", System.currentTimeMillis()))
        if (id == currentId) openInternal(all.maxByOrNull { it.updated }!!.id) else refreshList()
        runCatching { store.save(all) }
    }

    fun requestTrust(s: Server) {
        scope.launch { try { Api(s).requestTrust(); status = Api(s).status() } catch (e: NetError) { notice = e.message } catch (_: IllegalStateException) { } }
    }

    fun revokeTrust(s: Server) {
        scope.launch { try { Api(s).revokeTrust(); status = Api(s).status() } catch (e: NetError) { notice = e.message } catch (_: IllegalStateException) { } }
    }

    fun loadModels(s: Server) {
        scope.launch { try { models = Api(s).appModels() } catch (_: NetError) { } catch (_: IllegalStateException) { } }
    }

    /** Выбрать модель на ПК. Программа на ПК переключается сама; ждём, пока движок снова станет готов. */
    fun selectModel(s: Server, id: String) {
        if (switching || id == models?.current) return
        switching = true; notice = "Переключаю модель на ПК, это может занять пару минут…"
        scope.launch {
            try {
                val a = Api(s)
                a.selectModel(id)
                repeat(100) {
                    kotlinx.coroutines.delay(3000)
                    val m = try { a.appModels() } catch (_: NetError) { null }
                    if (m != null) { models = m; if (m.ready && m.current == id) { notice = null; return@launch } }
                }
                notice = "Модель не успела загрузиться. Проверьте программу на ПК."
            } catch (e: NetError) {
                notice = if (e is NetError.Http && e.code == 409) "Сначала дождитесь окончания задач." else (e.message ?: "Не удалось переключить модель")
            } finally { switching = false; loadStatus(s) }
        }
    }
    var status by mutableStateOf<ServerStatus?>(null)
        private set
    var busy by mutableStateOf(false)
        private set
    var lastOutcome by mutableStateOf<Boolean?>(null)
        private set
    private var job: Job? = null
    private var current: Msg? = null
    private var api: Api? = null

    val model: String? get() = status?.model

    fun loadStatus(s: Server) {
        scope.launch {
            try { status = Api(s).status() } catch (_: NetError) { status = null } catch (_: IllegalStateException) { }
        }
    }

    /** Вложение — текстовый файл: его содержимое добавляется к запросу. Бинарные файлы и картинки пока не поддерживаются. */
    fun send(s: Server, typed: String, attachment: Pair<String, String>?) {
        val text = if (attachment == null) typed else typed.trim() + "\n\n[Вложенный файл: ${attachment.first}]\n```\n${attachment.second}\n```"
        send(s, text)
    }

    fun send(s: Server, text: String) {
        if (busy || text.isBlank()) return
        val history = messages.filter { it.error == null && it.text.isNotEmpty() }.map { (if (it.fromUser) "user" else "assistant") to it.text }
        messages.add(Msg(true, text.trim()))
        persist()
        val reply = Msg(false).also { it.streaming = true }
        messages.add(reply)
        current = reply
        busy = true; lastOutcome = null
        val a = try { Api(s) } catch (e: IllegalStateException) { reply.error = "Ключ устройства утерян, привяжите устройство заново"; reply.streaming = false; busy = false; return }
        api = a
        job = scope.launch {
            try {
                val id = a.createTask(text.trim(), text.trim().take(60), history, fast)
                reply.taskId = id
                onTaskStarted()
                val v = follow(a, id) { applyEvents(reply, it.events) }
                if (reply.text.isEmpty() && !v.result.isNullOrEmpty()) reply.text = v.result
                when (v.info.status) {
                    "done" -> lastOutcome = true
                    "cancelled" -> reply.error = "Остановлено"
                    else -> { reply.error = v.info.error.ifEmpty { "Ошибка выполнения" }; lastOutcome = false }
                }
            } catch (e: NetError) {
                reply.error = when {
                    e is NetError.Http && e.code == 429 -> "Сервер занят. Повторите позже."
                    e is NetError.Http && e.code == 404 -> "Задача пропала на сервере (сервер перезапускали?)"
                    else -> (e.message ?: "Ошибка") + (if (reply.taskId != null) ". Задача продолжает выполняться на сервере, смотрите вкладку «Задачи»." else "")
                }
                lastOutcome = false
            } finally {
                reply.streaming = false; reply.approval = null; busy = false
                persist()
            }
        }
    }

    /** Остановить: сначала просим сервер отменить задачу (агент прервётся), слежение завершится само по событию. */
    fun stop() {
        val id = current?.taskId
        val a = api
        if (id != null && a != null) scope.launch { try { a.cancel(id) } catch (_: NetError) { } } else job?.cancel()
    }

    fun approve(m: Msg, ok: Boolean, code: String) {
        val p = m.approval ?: return
        val id = m.taskId ?: return
        val a = api ?: return
        scope.launch {
            try {
                a.approve(id, p.id, ok, code)
                m.approvalError = null
            } catch (e: NetError) {
                m.approvalError = when {
                    e is NetError.Http && e.code == 403 -> "Код не подошёл. Введите свежий код из Authenticator."
                    e is NetError.Http && e.code == 429 -> "Слишком много неверных кодов. Подождите 15 минут."
                    e is NetError.Http && e.code == 409 -> { m.approval = null; "Запрос уже закрыт" }
                    else -> e.message
                }
            }
        }
    }

    fun clear() { stop(); messages.clear(); lastOutcome = null; persist() }

    // должен стоять после всех свойств класса: иначе они ещё не созданы
    init {
        if (all.isEmpty()) all.add(StoredChat(java.util.UUID.randomUUID().toString().take(8), "Новый чат", System.currentTimeMillis()))
        openInternal(all.maxByOrNull { it.updated }!!.id)
    }
}
