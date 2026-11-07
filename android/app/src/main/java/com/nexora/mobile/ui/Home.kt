package com.nexora.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.clickable
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Dns
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Chat
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.nexora.mobile.net.Conn

fun Conn.robot(): RobotState = when (this) {
    is Conn.Online -> RobotState.Idle
    is Conn.Connecting -> RobotState.Connecting
    is Conn.Offline, is Conn.NoServer -> RobotState.Offline
    is Conn.AuthRequired, is Conn.Untrusted -> RobotState.Error
    is Conn.Locked -> RobotState.Offline
}

fun Conn.title(): String = when (this) {
    is Conn.Online -> "Подключено"
    is Conn.Connecting -> "Подключаюсь…"
    is Conn.Offline -> "Нет связи с сервером"
    is Conn.NoServer -> "Сервер не подключён"
    is Conn.Locked -> "Приложение заблокировано"
    is Conn.AuthRequired -> "Нужна повторная привязка"
    is Conn.Untrusted -> "Сертификат сервера изменился"
}

fun Conn.color(): Color = when (this) {
    is Conn.Online -> C.Ok
    is Conn.Connecting -> C.Warn
    is Conn.NoServer, is Conn.Offline, is Conn.Locked -> C.Muted
    is Conn.AuthRequired, is Conn.Untrusted -> C.Err
}

@Composable
fun HomeScreen(conn: Conn, userName: String, animated: Boolean, onConnect: () -> Unit, onChat: () -> Unit, onSettings: () -> Unit, recent: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Row(Modifier.fillMaxWidth().padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("✦ NEXORA", color = C.Text, fontSize = 24.sp, fontWeight = FontWeight.Bold, letterSpacing = 2.sp)
                Text("Мобильный агент", color = C.Muted, fontSize = 13.sp)
            }
            Icon(Icons.Filled.Settings, "Настройки", tint = C.Text, modifier = Modifier.size(28.dp).clickable(onClick = onSettings))
        }
        Spacer(Modifier.height(16.dp))
        Box(
            Modifier.size(230.dp).clip(CircleShape)
                .background(Brush.radialGradient(listOf(C.Violet.copy(alpha = 0.55f), C.Bg))),
            contentAlignment = Alignment.Center,
        ) { Robot(conn.robot(), animated, Modifier.size(200.dp)) }
        Spacer(Modifier.height(12.dp))
        Text(if (userName.isNotBlank()) "Привет, $userName! 👋" else "Привет! 👋", color = C.Text, fontSize = 24.sp, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(8.dp))
        StatusLine(conn.color(), conn.title())
        val sub = when (conn) {
            is Conn.Online -> "${conn.server.name.ifBlank { conn.server.host }} · ${conn.server.label}"
            is Conn.Offline -> "${conn.server.label}: ${conn.reason}. Повтор через ${conn.retryInSec} с"
            is Conn.Connecting -> conn.server.label
            is Conn.AuthRequired -> "Сервер не принимает это устройство. Возможно, оно отозвано. Привяжите его заново." + (if (conn.detail.isNotEmpty()) "\n\nПодробности: ${conn.detail}" else "")
            is Conn.Untrusted -> "Не подключайтесь, пока не убедитесь, что это ваш сервер."
            is Conn.Locked -> "Подтвердите биометрией или PIN экрана."
            Conn.NoServer -> "Подключитесь к серверу, чтобы начать работу."
        }
        Text(sub, color = C.Muted, fontSize = 14.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp).widthIn(max = 320.dp))
        Spacer(Modifier.height(20.dp))
        when (conn) {
            is Conn.Locked -> PrimaryButton("Разблокировать", Icons.Filled.Dns, onClick = onConnect)
            is Conn.Online -> PrimaryButton("Открыть чат", Icons.Filled.Chat, onClick = onChat)
            is Conn.Offline, is Conn.Connecting -> PrimaryButton("Повторить подключение", Icons.Filled.Dns, onClick = onConnect)
            else -> PrimaryButton("Подключиться к серверу", Icons.Filled.Dns, onClick = onConnect)
        }
        Spacer(Modifier.height(10.dp))
        SecondaryButton("Настройки агента", Icons.Filled.Settings, onClick = onSettings)
        recent()
        Spacer(Modifier.height(24.dp))
    }
}
