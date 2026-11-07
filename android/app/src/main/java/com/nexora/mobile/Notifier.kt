package com.nexora.mobile

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.nexora.mobile.net.Conn

/**
 * Уведомления. Текст всегда общий («Задача завершена»), без команд, путей и данных: на заблокированном экране
 * показывается публичная версия, подробности только внутри приложения (VISIBILITY_PRIVATE).
 */
object Notifier {
    const val CH = "events"
    private var id = 100

    private fun ensureChannel(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CH) == null) {
            nm.createNotificationChannel(NotificationChannel(CH, "События агента", NotificationManager.IMPORTANCE_DEFAULT).apply {
                lockscreenVisibility = Notification.VISIBILITY_PRIVATE
                setShowBadge(false)
            })
        }
    }

    fun show(ctx: Context, title: String) {
        if (Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        ensureChannel(ctx)
        val open = PendingIntent.getActivity(ctx, 0, Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE)
        val public = NotificationCompat.Builder(ctx, CH).setSmallIcon(android.R.drawable.stat_notify_chat).setContentTitle("Nexora Agent").build()
        val n = NotificationCompat.Builder(ctx, CH)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle(title)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(public)
            .build()
        ctx.getSystemService(NotificationManager::class.java).notify(id++, n)
    }

    fun onConnChange(ctx: Context, prefs: Prefs, old: Conn, new: Conn) {
        if (old is Conn.Online && new is Conn.Offline && prefs.notifyOffline) show(ctx, "Сервер отключился")
    }
}
