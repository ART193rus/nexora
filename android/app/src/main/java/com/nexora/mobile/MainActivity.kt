package com.nexora.mobile

import android.Manifest
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ChatBubbleOutline
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material.icons.filled.TaskAlt
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.nexora.mobile.net.Conn
import com.nexora.mobile.ui.BottomBar
import com.nexora.mobile.ui.C
import com.nexora.mobile.ui.ChatScreen
import com.nexora.mobile.ui.ConnectScreen
import com.nexora.mobile.ui.HomeScreen
import com.nexora.mobile.ui.MoreScreen
import com.nexora.mobile.ui.NexoraTheme
import com.nexora.mobile.ui.RecentTasks
import com.nexora.mobile.ui.TasksScreen

class MainActivity : ComponentActivity() {
    private val app get() = application as NexoraApp

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // окно рисуется под системными панелями, отступы (в том числе под клавиатуру) задаёт Compose один раз
        androidx.core.view.WindowCompat.setDecorFitsSystemWindows(window, false)
        applySecure()
        setContent { NexoraTheme { Root() } }
    }

    private fun applySecure() {
        if (app.prefs.secureScreen) window.addFlags(WindowManager.LayoutParams.FLAG_SECURE) else window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
    }

    override fun onStart() {
        super.onStart(); app.conn.onForeground(true); app.watcher.onForeground(true)
        if (app.servers.active()?.enrolled == true) com.nexora.mobile.data.Gate.prompt(this) { if (it) { app.conn.retryNow(); app.watcher.refreshNow() } }
    }
    override fun onStop() { super.onStop(); app.conn.onForeground(false); app.watcher.onForeground(false) }

    @Composable
    private fun Root() {
        val conn by app.conn.state.collectAsState()
        var tab by remember { mutableIntStateOf(0) }
        var connecting by remember { mutableStateOf(false) }
        var rev by remember { mutableIntStateOf(0) }
        rev.let { }

        val perm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
        LaunchedEffect(Unit) {
            if (Build.VERSION.SDK_INT >= 33) perm.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        // модели подгружаются, когда сервер стал доступен
        LaunchedEffect(conn is Conn.Online) { (conn as? Conn.Online)?.let { app.chat.loadStatus(it.server); app.chat.loadModels(it.server); app.watcher.refreshNow() } }
        // ссылка nexora:// из браузера открывает экран подключения
        LaunchedEffect(Unit) { if (intent?.data?.scheme == "nexora") connecting = true }

        BackHandler(connecting || tab != 0) { if (connecting) connecting = false else tab = 0 }

        val imeUp = WindowInsets.ime.getBottom(androidx.compose.ui.platform.LocalDensity.current) > 0
        Column(Modifier.fillMaxSize().background(C.Bg).statusBarsPadding().imePadding().then(if (connecting) Modifier.navigationBarsPadding() else Modifier)) {
            Box(Modifier.weight(1f)) {
                if (connecting) {
                    ConnectScreen(app, conn, onBack = { connecting = false }, onDone = { connecting = false; tab = 0 })
                } else when (tab) {
                    0 -> HomeScreen(
                        conn, app.prefs.userName, app.prefs.animations,
                        onConnect = { if (conn is Conn.Locked) com.nexora.mobile.data.Gate.prompt(this@MainActivity) { if (it) app.conn.retryNow() } else if (conn is Conn.Offline || conn is Conn.Connecting) app.conn.retryNow() else connecting = true },
                        onChat = { tab = 1 }, onSettings = { tab = 3 },
                        recent = { RecentTasks(app.watcher.tasks) { id -> (conn as? Conn.Online)?.let { app.tasks.open(it.server, id); tab = 2 } } },
                    )
                    1 -> ChatScreen(app.chat, conn, app.prefs.animations, onConnect = { connecting = true })
                    2 -> TasksScreen(app, conn)
                    else -> MoreScreen(app, conn, onConnect = { connecting = true }, onChange = { applySecure(); rev++ })
                }
            }
            if (!connecting && !imeUp) BottomBar(
                listOf(
                    Triple("Главная", Icons.Filled.Home, tab == 0),
                    Triple("Чат", Icons.Filled.ChatBubbleOutline, tab == 1),
                    Triple("Задачи", Icons.Filled.TaskAlt, tab == 2),
                    Triple("Ещё", Icons.Filled.MoreHoriz, tab == 3),
                ),
            ) { tab = it }
        }
    }
}
