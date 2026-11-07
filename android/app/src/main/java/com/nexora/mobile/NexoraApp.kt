package com.nexora.mobile

import android.app.Application
import android.content.Context
import com.nexora.mobile.data.ServerStore
import com.nexora.mobile.net.Connection

/** Настройки, которые нужны сразу всем экранам. */
class Prefs(ctx: Context) {
    private val p = ctx.getSharedPreferences("prefs", Context.MODE_PRIVATE)
    var animations: Boolean get() = p.getBoolean("anim", true); set(v) { p.edit().putBoolean("anim", v).apply() }
    var notifyDone: Boolean get() = p.getBoolean("n_done", true); set(v) { p.edit().putBoolean("n_done", v).apply() }
    var notifyError: Boolean get() = p.getBoolean("n_err", true); set(v) { p.edit().putBoolean("n_err", v).apply() }
    var notifyApprove: Boolean get() = p.getBoolean("n_appr", true); set(v) { p.edit().putBoolean("n_appr", v).apply() }
    var notifyOffline: Boolean get() = p.getBoolean("n_off", true); set(v) { p.edit().putBoolean("n_off", v).apply() }
    var secureScreen: Boolean get() = p.getBoolean("secure", true); set(v) { p.edit().putBoolean("secure", v).apply() }
    var fastChat: Boolean get() = p.getBoolean("fast", true); set(v) { p.edit().putBoolean("fast", v).apply() }
    var userName: String get() = p.getString("user", "") ?: ""; set(v) { p.edit().putString("user", v).apply() }
}

class NexoraApp : Application() {
    lateinit var servers: ServerStore
    lateinit var conn: Connection
    lateinit var prefs: Prefs
    lateinit var chat: ChatModel
    lateinit var watcher: Watcher
    lateinit var tasks: TasksModel

    override fun onCreate() {
        super.onCreate()
        servers = ServerStore(this)
        prefs = Prefs(this)
        conn = Connection(this) { old, new -> Notifier.onConnChange(this, prefs, old, new) }
        watcher = Watcher(this)
        chat = ChatModel(com.nexora.mobile.data.ChatStore(this), prefs, onTaskStarted = { watcher.kick(); watcher.refreshNow() })
        tasks = TasksModel(onChanged = { watcher.refreshNow() })
        conn.setServer(servers.active())
    }
}
