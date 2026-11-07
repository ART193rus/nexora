package com.nexora.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.nexora.mobile.NexoraApp
import com.nexora.mobile.net.Conn
import com.nexora.mobile.net.TaskInfo
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

fun statusLabel(s: String) = when (s) {
    "queued" -> "В очереди"; "running" -> "Выполняется"; "waiting" -> "Ждёт подтверждения"
    "done" -> "Завершена"; "error" -> "Ошибка"; "cancelled" -> "Остановлена"; else -> s
}

fun statusColor(s: String): Color = when (s) {
    "running" -> C.Purple; "waiting" -> C.Warn; "done" -> C.Ok; "error" -> C.Err; else -> C.Muted
}

private fun hm(ts: Long) = if (ts <= 0) "" else SimpleDateFormat("d MMM, HH:mm", Locale("ru")).format(Date(ts * 1000))

@Composable
fun TasksScreen(app: NexoraApp, conn: Conn) {
    val w = app.watcher
    val model = app.tasks
    val online = conn as? Conn.Online
    val openId = model.openId
    if (openId != null && online != null) {
        TaskDetail(app, online)
        return
    }
    var tab by remember { mutableIntStateOf(0) }
    val tabs = listOf("Активные", "Ожидающие", "Завершённые", "С ошибкой")
    fun inTab(t: TaskInfo) = when (tab) {
        0 -> t.status == "running" || t.status == "waiting"
        1 -> t.status == "queued"
        2 -> t.status == "done" || t.status == "cancelled"
        else -> t.status == "error"
    }
    Screen("Задачи агента") {
        Column(Modifier.fillMaxSize()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                tabs.forEachIndexed { i, t ->
                    val n = w.tasks.count { x -> when (i) { 0 -> x.status == "running" || x.status == "waiting"; 1 -> x.status == "queued"; 2 -> x.status == "done" || x.status == "cancelled"; else -> x.status == "error" } }
                    Box(
                        Modifier.clip(RoundedCornerShape(14.dp)).background(if (tab == i) C.Violet else C.Panel).clickable { tab = i }.padding(horizontal = 10.dp, vertical = 8.dp),
                    ) { Text(if (n > 0) "$t $n" else t, color = C.Text, fontSize = 12.sp, fontWeight = if (tab == i) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1) }
                }
            }
            Spacer(Modifier.height(10.dp))
            val shown = w.tasks.filter(::inTab)
            when {
                online == null -> Empty("Нет подключения к серверу")
                !w.loaded -> Empty("Загружаю…")
                shown.isEmpty() -> Empty(if (w.tasks.isEmpty()) "Задач пока нет.\nОтправьте запрос агенту в чате." else "В этом разделе пусто")
                else -> LazyColumn(contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 4.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    items(shown, key = { it.id }) { t -> TaskCard(t) { model.open(online.server, t.id) } }
                }
            }
        }
    }
}

@Composable
private fun Empty(text: String) {
    Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.TopCenter) { Text(text, color = C.Muted, fontSize = 14.sp, lineHeight = 20.sp) }
}

@Composable
fun TaskCard(t: TaskInfo, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().panel().clickable(onClick = onClick).padding(14.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(t.title, color = C.Text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, modifier = Modifier.weight(1f))
        }
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Dot(statusColor(t.status), 8); Spacer(Modifier.size(6.dp))
            Text(statusLabel(t.status), color = statusColor(t.status), fontSize = 13.sp)
            Spacer(Modifier.size(10.dp))
            val when_ = if (t.started > 0) "Начало ${hm(t.started)}" else "Создана ${hm(t.created)}"
            Text(when_, color = C.Muted, fontSize = 12.sp)
        }
        // процентов нет: сервер не знает общего числа шагов, показываем только число выполненных
        if (t.steps > 0) Text("Выполнено шагов: ${t.steps}", color = C.Muted, fontSize = 12.sp, modifier = Modifier.padding(top = 4.dp))
        if (t.pending != null) Text("Нужно ваше подтверждение", color = C.Warn, fontSize = 13.sp, modifier = Modifier.padding(top = 4.dp))
    }
}

@Composable
private fun TaskDetail(app: NexoraApp, online: Conn.Online) {
    val m = app.tasks
    val info = m.info
    val msg = m.msg
    val ctx = androidx.compose.ui.platform.LocalContext.current
    val save = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("text/markdown")) { uri ->
        val text = m.result ?: msg.text
        if (uri != null) runCatching { ctx.contentResolver.openOutputStream(uri)?.use { it.write(text.toByteArray(Charsets.UTF_8)) } }
    }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.ArrowBack, "Назад", tint = C.Text, modifier = Modifier.size(26.dp).clickable { m.close() })
            Spacer(Modifier.size(12.dp))
            Text(info?.title ?: "Задача", color = C.Text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, modifier = Modifier.weight(1f))
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            if (info != null) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Dot(statusColor(info.status)); Spacer(Modifier.size(8.dp)); Text(statusLabel(info.status), color = statusColor(info.status), fontSize = 15.sp)
                }
                Text(buildString {
                    append("Создана ${hm(info.created)}")
                    if (info.started > 0) append(" · начата ${hm(info.started)}")
                    if (info.finished > 0) append(" · завершена ${hm(info.finished)}")
                }, color = C.Muted, fontSize = 12.sp, modifier = Modifier.padding(top = 4.dp))
                if (info.steps > 0) Text("Выполнено шагов: ${info.steps}", color = C.Muted, fontSize = 12.sp)
            }
            msg.approval?.let { ApprovalCard(it, msg.approvalError) { ok, code -> m.approve(ok, code) } }
            SectionTitle("Журнал")
            if (msg.actions.isEmpty()) Text("Действий с инструментами не было.", color = C.Muted, fontSize = 13.sp)
            msg.actions.forEach { ActionRow(it) }
            if (msg.text.isNotEmpty() || m.result != null) {
                SectionTitle("Результат")
                Column(Modifier.fillMaxWidth().panel().padding(14.dp)) { Text(m.result ?: msg.text, color = C.Text, fontSize = 15.sp, lineHeight = 21.sp) }
            }
            (info?.error?.takeIf { it.isNotEmpty() } ?: m.error)?.let { Text(it, color = C.Err, fontSize = 14.sp, modifier = Modifier.padding(top = 12.dp)) }
            Spacer(Modifier.height(16.dp))
        }
        Column(Modifier.padding(16.dp)) {
            if (!(m.result ?: msg.text).isNullOrBlank() && info?.active != true) { SecondaryButton("Сохранить результат в файл") { save.launch((info?.title ?: "результат").take(40).replace(Regex("[^\\p{L}\\p{N} _-]"), "") + ".md") }; Spacer(Modifier.height(8.dp)) }
            if (info?.active == true) SecondaryButton("Остановить", color = C.Err) { m.cancel() }
            else if (info != null) PrimaryButton("Повторить") { m.retry(online.server, info.id) }
        }
    }
}

@Composable
fun ActionRow(a: com.nexora.mobile.Action) {
    var open by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(bottom = 6.dp).panel(12.dp).clickable(enabled = a.output.isNotEmpty()) { open = !open }.padding(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val c = when (a.status) { "ok" -> C.Ok; "running" -> C.Purple; "denied" -> C.Warn; else -> C.Err }
            Dot(c, 8); Spacer(Modifier.size(8.dp))
            Text(a.title, color = C.Text, fontSize = 13.sp, maxLines = if (open) 6 else 2, modifier = Modifier.weight(1f))
        }
        if (open && a.output.isNotEmpty()) Text(a.output, color = C.Muted, fontSize = 12.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(top = 8.dp))
    }
}

/** Запрос подтверждения. Опасные действия (удаление, права администратора, боевой сервер) требуют свежий код Authenticator. */
@Composable
fun ApprovalCard(p: com.nexora.mobile.net.Pending, error: String?, onAnswer: (Boolean, String) -> Unit) {
    var code by remember(p.id) { mutableStateOf("") }
    Column(Modifier.fillMaxWidth().padding(vertical = 10.dp).panel(fill = C.Warn.copy(alpha = 0.10f), border = C.Warn.copy(alpha = 0.6f)).padding(14.dp)) {
        Text(if (p.dangerous) "Опасное действие: нужно подтверждение" else "Требуется подтверждение", color = C.Warn, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        Text(p.title, color = C.Text, fontSize = 15.sp, modifier = Modifier.padding(top = 6.dp))
        if (p.detail.isNotEmpty()) Text(p.detail.take(1200), color = C.Muted, fontSize = 12.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(top = 6.dp))
        if (p.dangerous) {
            Spacer(Modifier.height(10.dp))
            Field("Код из Google Authenticator", code, { code = it.filter(Char::isDigit).take(6) }, placeholder = "6 цифр", number = true)
        }
        error?.let { Text(it, color = C.Err, fontSize = 13.sp, modifier = Modifier.padding(top = 8.dp)) }
        Spacer(Modifier.height(12.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.weight(1f)) { SecondaryButton("Отклонить", color = C.Err) { onAnswer(false, "") } }
            Box(Modifier.weight(1f)) { PrimaryButton("Разрешить", enabled = !p.dangerous || code.length == 6) { onAnswer(true, code) } }
        }
    }
}

/** Быстрый доступ к последним задачам на главной. */
@Composable
fun RecentTasks(tasks: List<TaskInfo>, onOpen: (Long) -> Unit) {
    if (tasks.isEmpty()) return
    Column(Modifier.fillMaxWidth()) {
        SectionTitle("Последние задачи")
        tasks.take(3).forEach { t ->
            Box(Modifier.padding(bottom = 8.dp)) { TaskCard(t) { onOpen(t.id) } }
        }
    }
}
