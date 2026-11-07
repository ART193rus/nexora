package com.nexora.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.app.NotificationCompat
import com.nexora.mobile.net.Api
import com.nexora.mobile.net.Conn
import com.nexora.mobile.net.NetError
import com.nexora.mobile.net.TaskInfo
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Следит за списком задач: обновляет экран «Задачи» и шлёт уведомления о переходах (завершена, ошибка, нужно подтверждение).
 * Пока есть активные задачи, держит приложение живым через служебное уведомление (foreground service), иначе уведомления
 * в фоне не придут. Сервис стартует только при открытом приложении (требование Android) и сам останавливается без активных задач.
 * Уведомления намеренно общие: без названий задач, команд и путей.
 */
class Watcher(private val app: NexoraApp) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    var tasks by mutableStateOf<List<TaskInfo>>(emptyList())
        private set
    var loaded by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    private var job: Job? = null
    private var prev: Map<Long, TaskInfo>? = null
    private var foreground = false
    private var serviceOn = false
    private var fails = 0

    fun onForeground(v: Boolean) { foreground = v; if (v) ensureRunning() }

    /** Вызывать, когда появилась (или может появиться) новая задача. */
    fun kick() { ensureRunning() }

    fun refreshNow() { scope.launch { tick() } }

    private fun ensureRunning() {
        if (job?.isActive == true) return
        job = scope.launch {
            while (true) {
                val c = app.conn.state.value
                val active = tasks.any { it.active }
                if (!foreground && !active) { job = null; return@launch }
                if (c is Conn.Online || !foreground) tick()
                delay(if (foreground) 4000 else 8000)
            }
        }
    }

    private suspend fun tick() {
        if (!com.nexora.mobile.data.Gate.unlocked) return
        val s = app.servers.active()?.takeIf { it.enrolled } ?: run { tasks = emptyList(); loaded = false; prev = null; return }
        try {
            val list = Api(s).tasks()
            fails = 0; error = null
            note(list)
            tasks = list; loaded = true
            syncService(list.any { it.active })
        } catch (e: NetError) {
            error = e.message
            if (++fails == 3 && tasks.any { it.active } && app.prefs.notifyOffline) Notifier.show(app, "Сервер отключился")
        } catch (_: IllegalStateException) {
        }
    }

    private fun note(list: List<TaskInfo>) {
        val old = prev
        prev = list.associateBy { it.id }
        if (old == null) return // первый снимок: прошлое не объявляем
        for (t in list) {
            val o = old[t.id]
            if (t.pending != null && o?.pending?.id != t.pending.id && app.prefs.notifyApprove) Notifier.show(app, "Требуется подтверждение")
            if (o != null && o.active && !t.active) {
                when {
                    t.status == "done" && app.prefs.notifyDone -> Notifier.show(app, "Задача завершена")
                    t.status == "error" && app.prefs.notifyError -> Notifier.show(app, "Задача завершилась с ошибкой")
                }
            }
        }
    }

    private fun syncService(anyActive: Boolean) {
        if (anyActive && !serviceOn && foreground) {
            runCatching { app.startForegroundService(Intent(app, WatchService::class.java)); serviceOn = true }
        } else if (!anyActive && serviceOn) {
            app.stopService(Intent(app, WatchService::class.java)); serviceOn = false
        }
    }
}

/** Служебное уведомление «Nexora следит за задачами»: держит процесс живым, пока задачи идут. */
class WatchService : Service() {
    override fun onBind(i: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val nm = getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel("watch") == null) nm.createNotificationChannel(NotificationChannel("watch", "Слежение за задачами", NotificationManager.IMPORTANCE_LOW).apply { lockscreenVisibility = Notification.VISIBILITY_SECRET })
        val n = NotificationCompat.Builder(this, "watch").setSmallIcon(android.R.drawable.stat_notify_sync).setContentTitle("Nexora: выполняются задачи")
            .setOngoing(true).setVisibility(NotificationCompat.VISIBILITY_SECRET).build()
        startForeground(7, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        return START_NOT_STICKY
    }
}
