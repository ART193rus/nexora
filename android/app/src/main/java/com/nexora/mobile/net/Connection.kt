package com.nexora.mobile.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import com.nexora.mobile.data.Server

/** Состояние связи с активным сервером. От него зависят робот, статус на главной и уведомления. */
sealed interface Conn {
    data object NoServer : Conn
    data class Connecting(val server: Server) : Conn
    data class Online(val server: Server, val device: String) : Conn
    data class Offline(val server: Server, val reason: String, val retryInSec: Int) : Conn

    /** Сервер несколько раз подряд молча закрыл соединение: устройство отозвано или привязка недействительна. Нужна повторная привязка. */
    data class AuthRequired(val server: Server, val detail: String = "") : Conn

    /** Ключ устройства заблокирован: нужно подтвердить биометрией или PIN экрана. */
    data class Locked(val server: Server) : Conn

    /** Сертификат сервера не тот, что при привязке. Автоматически не повторяется. */
    data class Untrusted(val server: Server) : Conn
}

/**
 * Держит соединение живым: пинг каждые 15 с, пока приложение на экране; при потере сети повторы с нарастающей паузой
 * (3, 6, 12, 30 с), мгновенный повтор при появлении сети и при возврате из фона.
 */
class Connection(private val ctx: Context, private val onEvent: (Conn, Conn) -> Unit) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow<Conn>(Conn.NoServer)
    val state: StateFlow<Conn> = _state.asStateFlow()

    private var server: Server? = null
    private var job: Job? = null
    private var foreground = false
    private var rejects = 0
    @Volatile private var kick = false

    private val cm = ctx.getSystemService(ConnectivityManager::class.java)
    private val netCb = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { kick = true }
    }

    init { runCatching { cm.registerDefaultNetworkCallback(netCb) } }

    fun setServer(s: Server?) {
        server = s
        rejects = 0
        restart()
    }

    fun onForeground(v: Boolean) {
        foreground = v
        if (v) { kick = true; restart() } else { job?.cancel(); job = null }
    }

    /** Пингануть сейчас (кнопка «Подключиться» / «Повторить»). */
    fun retryNow() { rejects = 0; restart() }

    private fun set(c: Conn) {
        val old = _state.value
        if (old != c) { _state.value = c; onEvent(old, c) }
    }

    private fun restart() {
        job?.cancel()
        val s = server
        if (s == null || !s.enrolled) { set(Conn.NoServer); return }
        if (!foreground) return
        job = scope.launch {
            if (_state.value !is Conn.Online) set(Conn.Connecting(s))
            var backoff = 0
            while (true) {
                if (!com.nexora.mobile.data.Gate.unlocked) { set(Conn.Locked(s)); waitOrKick(1); continue }
                try {
                    val dev = Api(s).ping()
                    rejects = 0; backoff = 0
                    set(Conn.Online(s, dev))
                    waitOrKick(15)
                } catch (e: NetError) {
                    when (e) {
                        is NetError.IdentityChanged -> { set(Conn.Untrusted(s)); return@launch }
                        is NetError.Rejected -> {
                            if (++rejects >= 3) { set(Conn.AuthRequired(s, e.detail)); return@launch }
                            set(Conn.Offline(s, "Сервер закрыл соединение (${e.detail.take(80)})", 3))
                            waitOrKick(3)
                        }
                        else -> {
                            val wait = intArrayOf(3, 6, 12, 30)[backoff.coerceAtMost(3)]
                            backoff++
                            set(Conn.Offline(s, e.message ?: "Нет связи", wait))
                            waitOrKick(wait)
                        }
                    }
                } catch (e: IllegalStateException) { // ключ в Keystore утерян
                    set(Conn.AuthRequired(s)); return@launch
                }
            }
        }
    }

    private suspend fun waitOrKick(sec: Int) {
        kick = false
        var left = sec * 4
        while (left-- > 0 && !kick) delay(250)
    }
}
