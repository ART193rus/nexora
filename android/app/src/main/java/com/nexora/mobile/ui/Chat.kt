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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.Send
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.nexora.mobile.ChatModel
import com.nexora.mobile.Msg
import com.nexora.mobile.net.Conn

private const val MAX_ATTACH = 60_000

@Composable
fun ChatScreen(chat: ChatModel, conn: Conn, animated: Boolean, onConnect: () -> Unit) {
    var input by remember { mutableStateOf("") }
    var showList by remember { mutableStateOf(false) }
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var attach by remember { mutableStateOf<Pair<String, String>?>(null) }
    var attachErr by remember { mutableStateOf<String?>(null) }
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            attachErr = null
            try {
                val cr = ctx.contentResolver
                val name = cr.query(uri, null, null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(c.getColumnIndexOrThrow(android.provider.OpenableColumns.DISPLAY_NAME)) else null } ?: "файл"
                val bytes = cr.openInputStream(uri)?.use { it.readNBytes(MAX_ATTACH + 1) } ?: ByteArray(0)
                if (bytes.size > MAX_ATTACH) attachErr = "Файл больше ${MAX_ATTACH / 1000} КБ: прикрепите меньший"
                else {
                    // строгая проверка: только текст в UTF-8, без нулевых байтов
                    val dec = Charsets.UTF_8.newDecoder().onMalformedInput(java.nio.charset.CodingErrorAction.REPORT).onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT)
                    val text = try { dec.decode(java.nio.ByteBuffer.wrap(bytes)).toString() } catch (_: Exception) { null }
                    if (text == null || text.contains('\u0000')) attachErr = "Это не текстовый файл. Сейчас поддерживается только текст (txt, md, csv, json, код)"
                    else attach = name to text
                }
            } catch (e: Exception) { attachErr = "Не удалось прочитать файл" }
        }
    }
    // Голосовой ввод через системное окно распознавания речи: своих разрешений на микрофон не нужно
    val voice = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.StartActivityForResult()) { r ->
        val text = r.data?.getStringArrayListExtra(android.speech.RecognizerIntent.EXTRA_RESULTS)?.firstOrNull()
        if (r.resultCode == android.app.Activity.RESULT_OK && !text.isNullOrBlank()) input = (input.trimEnd() + " " + text).trim()
    }
    fun startVoice() {
        val i = android.content.Intent(android.speech.RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(android.speech.RecognizerIntent.EXTRA_LANGUAGE_MODEL, android.speech.RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(android.speech.RecognizerIntent.EXTRA_LANGUAGE, "ru-RU")
            .putExtra(android.speech.RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
            .putExtra(android.speech.RecognizerIntent.EXTRA_PROMPT, "Говорите")
        try { voice.launch(i) } catch (_: Exception) { chat.notice = "На телефоне нет службы распознавания речи" }
    }
    val list = rememberLazyListState()
    val online = conn as? Conn.Online
    val robotState = when {
        conn !is Conn.Online -> conn.robot()
        chat.busy -> if (chat.messages.lastOrNull()?.text.isNullOrEmpty()) RobotState.Thinking else RobotState.Executing
        chat.lastOutcome == false -> RobotState.Error
        else -> RobotState.Idle
    }
    LaunchedEffect(chat.messages.size, chat.messages.lastOrNull()?.text?.length) {
        if (chat.messages.isNotEmpty()) list.scrollToItem(chat.messages.lastIndex)
    }

    Column(Modifier.fillMaxSize()) {
        // верх: чаты, модель на ПК, режим, аватар
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.Menu, "Чаты", tint = C.Text, modifier = Modifier.size(40.dp).clip(CircleShape).clickable { showList = !showList }.padding(8.dp))
            Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                Text(chat.chats.firstOrNull { it.id == chat.currentId }?.title ?: "Чат", color = C.Text, fontSize = 16.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, maxLines = 1)
                StatusLine(conn.color(), conn.title())
            }
            Icon(Icons.Filled.Add, "Новый чат", tint = if (chat.busy) C.Muted else C.PurpleHi, modifier = Modifier.size(40.dp).clip(CircleShape).clickable(enabled = !chat.busy) { chat.newChat(); showList = false }.padding(8.dp))
            Robot(robotState, animated, Modifier.size(52.dp))
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            var open by remember { mutableStateOf(false) }
            val m = chat.models
            val curLabel = m?.models?.firstOrNull { it.first == m.current }?.second ?: m?.current ?: chat.model ?: "не запущена"
            Box(Modifier.weight(1f)) {
                Row(
                    Modifier.clip(RoundedCornerShape(12.dp)).background(C.Panel).clickable(enabled = online != null && !chat.switching && (m?.models?.size ?: 0) > 1) { open = true }.padding(horizontal = 12.dp, vertical = 9.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(if (chat.switching) "Переключаю…" else "Модель: $curLabel", color = C.Text, fontSize = 13.sp, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                    if ((m?.models?.size ?: 0) > 1) Icon(Icons.Filled.ExpandMore, null, tint = C.Muted)
                }
                DropdownMenu(open, { open = false }) {
                    m?.models?.forEach { (id, label) ->
                        DropdownMenuItem({ Text(label + if (id == m.current) "  ✓" else "") }, { open = false; online?.let { chat.selectModel(it.server, id) } })
                    }
                }
            }
            Row(Modifier.clip(RoundedCornerShape(12.dp)).background(C.Panel).clickable { chat.useFast(!chat.fast) }.padding(horizontal = 12.dp, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(if (chat.fast) "⚡ Быстро" else "🛠 Агент", color = if (chat.fast) C.Teal else C.PurpleHi, fontSize = 13.sp)
            }
        }
        chat.notice?.let { Text(it, color = C.Warn, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) }
        if (!chat.fast) Text("Режим агента: длинная подсказка с инструментами, на слабом процессоре ответ может идти минуты.", color = C.Muted, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp))

        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (showList) {
                ChatList(chat) { showList = false }
            } else if (online == null) {
                Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("Нет подключения к серверу", color = C.Text, fontSize = 17.sp)
                    Spacer(Modifier.size(12.dp))
                    PrimaryButton("Подключиться", onClick = onConnect)
                }
            } else if (chat.messages.isEmpty()) {
                Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
                    Text("Напишите сообщение агенту.\nОтвет считается на сервере, телефон только показывает его.", color = C.Muted, fontSize = 14.sp, lineHeight = 20.sp)
                }
            } else {
                LazyColumn(state = list, contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    itemsIndexed(chat.messages) { _, m -> Bubble(m) { ok, code -> chat.approve(m, ok, code) } }
                }
            }
        }

        attachErr?.let { Text(it, color = C.Err, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp)) }
        attach?.let { a ->
            Row(Modifier.padding(horizontal = 12.dp).clip(RoundedCornerShape(12.dp)).background(C.Panel2).padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("📎 ${a.first} (${a.second.length} зн.)", color = C.Text, fontSize = 12.sp, maxLines = 1)
                Text("  ✕", color = C.Muted, fontSize = 14.sp, modifier = Modifier.clickable { attach = null })
            }
        }
        // низ: ввод. Вложения файлов появятся вместе с файловым API сервера.
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f).clip(RoundedCornerShape(24.dp)).background(C.Panel).padding(horizontal = 16.dp, vertical = 12.dp)) {
                if (input.isEmpty()) Text("Напишите сообщение…", color = C.Muted, fontSize = 15.sp)
                BasicTextField(input, { input = it }, textStyle = TextStyle(color = C.Text, fontSize = 15.sp), cursorBrush = SolidColor(C.PurpleHi), maxLines = 5, modifier = Modifier.fillMaxWidth())
            }
            Icon(Icons.Filled.AttachFile, "Прикрепить файл", tint = C.PurpleHi, modifier = Modifier.size(44.dp).clip(CircleShape).clickable(enabled = !chat.busy) { picker.launch(arrayOf("text/*", "application/json", "application/xml", "application/x-yaml", "application/octet-stream")) }.padding(10.dp))
            Icon(Icons.Filled.Mic, "Голосовой ввод", tint = C.PurpleHi, modifier = Modifier.size(44.dp).clip(CircleShape).clickable(enabled = !chat.busy) { startVoice() }.padding(10.dp))
            Spacer(Modifier.size(4.dp))
            val canSend = online != null && (input.isNotBlank() || attach != null) && !chat.busy
            Box(
                Modifier.size(48.dp).clip(CircleShape).background(if (chat.busy || canSend) C.Btn else SolidColor(C.Panel2)).clickable(enabled = chat.busy || canSend) {
                    if (chat.busy) chat.stop() else { chat.send(online!!.server, input, attach); input = ""; attach = null }
                },
                contentAlignment = Alignment.Center,
            ) { Icon(if (chat.busy) Icons.Filled.Stop else Icons.Filled.Send, if (chat.busy) "Остановить" else "Отправить", tint = androidx.compose.ui.graphics.Color.White) }
        }
    }
}

@Composable
private fun Bubble(m: Msg, onApprove: (Boolean, String) -> Unit) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = if (m.fromUser) Arrangement.End else Arrangement.Start) {
        Column(
            Modifier.widthIn(max = 340.dp).clip(RoundedCornerShape(18.dp)).background(if (m.fromUser) C.Violet else C.Panel).padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            if (m.thinking.isNotEmpty() && m.text.isEmpty() && m.streaming) Text("Думает…", color = C.Muted, fontSize = 13.sp)
            m.actions.forEach { ActionRow(it) }
            m.approval?.let { ApprovalCard(it, m.approvalError, onApprove) }
            if (m.text.isNotEmpty()) Text(if (m.fromUser && m.text.length > 1500) m.text.take(500) + "\n… (текст файла свёрнут, модель его получила)" else m.text, color = C.Text, fontSize = 15.sp, lineHeight = 21.sp)
            if (m.text.isEmpty() && m.streaming && m.thinking.isEmpty() && m.actions.isEmpty()) Text("…", color = C.Muted, fontSize = 15.sp)
            m.error?.let { Text(it, color = C.Err, fontSize = 14.sp) }
        }
    }
}

@Composable
private fun ChatList(chat: ChatModel, onClose: () -> Unit) {
    var confirm by remember { mutableStateOf<String?>(null) }
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 8.dp)) {
        PrimaryButton("Новый чат", Icons.Filled.Add, enabled = !chat.busy) { chat.newChat(); onClose() }
        Spacer(Modifier.size(10.dp))
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            itemsIndexed(chat.chats) { _, c ->
                Row(
                    Modifier.fillMaxWidth().panel(14.dp, fill = if (c.id == chat.currentId) C.Panel2 else C.Panel).clickable(enabled = !chat.busy) { chat.openChat(c.id); onClose() }.padding(14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(c.title, color = C.Text, fontSize = 15.sp, maxLines = 1)
                        Text(java.text.SimpleDateFormat("d MMM, HH:mm", java.util.Locale("ru")).format(java.util.Date(c.updated)), color = C.Muted, fontSize = 12.sp)
                    }
                    if (confirm == c.id) {
                        Text("Удалить?", color = C.Err, fontSize = 14.sp, modifier = Modifier.clickable { chat.deleteChat(c.id); confirm = null }.padding(8.dp))
                    } else {
                        Icon(Icons.Filled.Delete, "Удалить", tint = C.Muted, modifier = Modifier.size(36.dp).clickable(enabled = !chat.busy) { confirm = c.id }.padding(6.dp))
                    }
                }
            }
        }
    }
}
