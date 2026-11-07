package com.nexora.mobile

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.nexora.mobile.data.Server
import com.nexora.mobile.net.Api
import com.nexora.mobile.net.NetError
import com.nexora.mobile.net.TaskInfo
import com.nexora.mobile.net.follow
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/** Открытая задача: журнал событий, ответ, результат и действия над ней. Сообщение [msg] заполняется теми же событиями, что и чат. */
class TasksModel(private val onChanged: () -> Unit) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    var openId by mutableStateOf<Long?>(null)
        private set
    var info by mutableStateOf<TaskInfo?>(null)
        private set
    var msg by mutableStateOf(Msg(false))
        private set
    var result by mutableStateOf<String?>(null)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    private var job: Job? = null
    private var api: Api? = null

    fun open(s: Server, id: Long) {
        close()
        openId = id; info = null; result = null; error = null; msg = Msg(false).also { it.taskId = id }
        val a = try { Api(s) } catch (e: IllegalStateException) { error = "Ключ устройства утерян"; return }
        api = a
        val m = msg
        job = scope.launch {
            try {
                follow(a, id) { v -> applyEvents(m, v.events); info = v.info; if (v.result != null) result = v.result }
            } catch (e: NetError) {
                error = if (e is NetError.Http && e.code == 404) "Задача пропала на сервере" else e.message
            }
        }
    }

    fun close() { job?.cancel(); job = null; openId = null; api = null }

    fun cancel() { val id = openId ?: return; val a = api ?: return; scope.launch { try { a.cancel(id); onChanged() } catch (e: NetError) { error = e.message } } }

    fun retry(s: Server, id: Long) {
        scope.launch {
            try { val n = Api(s).retry(id); onChanged(); open(s, n) } catch (e: NetError) { error = e.message } catch (_: IllegalStateException) { }
        }
    }

    fun approve(ok: Boolean, code: String) {
        val p = msg.approval ?: return
        val id = openId ?: return
        val a = api ?: return
        val m = msg
        scope.launch {
            try { a.approve(id, p.id, ok, code); m.approvalError = null } catch (e: NetError) {
                m.approvalError = when {
                    e is NetError.Http && e.code == 403 -> "Код не подошёл. Введите свежий код из Authenticator."
                    e is NetError.Http && e.code == 429 -> "Слишком много неверных кодов. Подождите 15 минут."
                    e is NetError.Http && e.code == 409 -> { m.approval = null; "Запрос уже закрыт" }
                    else -> e.message
                }
            }
        }
    }
}
