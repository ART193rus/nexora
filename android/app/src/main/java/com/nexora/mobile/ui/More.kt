package com.nexora.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import kotlinx.coroutines.launch
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.nexora.mobile.NexoraApp
import com.nexora.mobile.net.Conn

@Composable
private fun Toggle(title: String, sub: String? = null, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, color = C.Text, fontSize = 15.sp)
            if (sub != null) Text(sub, color = C.Muted, fontSize = 12.sp, modifier = Modifier.padding(top = 2.dp))
        }
        Switch(checked, onChange, colors = SwitchDefaults.colors(checkedTrackColor = C.Purple, checkedThumbColor = androidx.compose.ui.graphics.Color.White, uncheckedTrackColor = C.Panel2, uncheckedBorderColor = C.Line))
    }
}

@Composable
private fun Item(title: String, sub: String? = null, color: androidx.compose.ui.graphics.Color = C.Text, onClick: (() -> Unit)? = null) {
    Column(Modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(horizontal = 14.dp, vertical = 12.dp)) {
        Text(title, color = color, fontSize = 15.sp)
        if (sub != null) Text(sub, color = C.Muted, fontSize = 12.sp, modifier = Modifier.padding(top = 2.dp))
    }
}

@Composable
fun MoreScreen(app: NexoraApp, conn: Conn, onConnect: () -> Unit, onChange: () -> Unit) {
    val p = app.prefs
    var anim by remember { mutableStateOf(p.animations) }
    var secure by remember { mutableStateOf(p.secureScreen) }
    var nDone by remember { mutableStateOf(p.notifyDone) }
    var nErr by remember { mutableStateOf(p.notifyError) }
    var nAppr by remember { mutableStateOf(p.notifyApprove) }
    var nOff by remember { mutableStateOf(p.notifyOffline) }
    var rev by remember { mutableStateOf(0) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    var confirmRemove by remember { mutableStateOf<String?>(null) }
    rev.let { }

    Screen("Ещё") {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            SectionTitle("Серверы")
            Column(Modifier.fillMaxWidth().panel()) {
                val list = app.servers.all().filter { it.enrolled }
                if (list.isEmpty()) Item("Серверов нет", "Привяжите устройство к Nexora Server")
                list.forEach { s ->
                    val active = s.id == app.servers.activeId
                    Item(
                        (s.name.ifBlank { s.host }) + if (active) " · активен" else "", s.label,
                        onClick = { app.servers.setActive(s.id); app.conn.setServer(s); rev++; onChange() },
                    )
                }
                Item("Подключить сервер", color = C.PurpleHi, onClick = onConnect)
            }

            SectionTitle("Сессии и устройство")
            Column(Modifier.fillMaxWidth().panel()) {
                val s = app.servers.active()
                Item("Это устройство", if (conn is Conn.Online) "Вход подтверждён сервером: ${conn.device}" else "Связи с сервером нет")
                if (s != null) {
                    Item(
                        "Выйти и забыть сервер", "Ключ удаляется с телефона, а если есть связь, устройство отзывается и на сервере.",
                        color = C.Err, onClick = { confirmRemove = s.id },
                    )
                }
            }
            confirmRemove?.let { id ->
                Column(Modifier.fillMaxWidth().padding(top = 8.dp).panel(fill = C.Err.copy(alpha = 0.10f), border = C.Err.copy(alpha = 0.5f)).padding(14.dp)) {
                    Text("Удалить ключ и забыть этот сервер? Для возврата понадобится новая привязка с кодом Authenticator.", color = C.Text, fontSize = 14.sp)
                    Spacer(Modifier.size(10.dp))
                    Row {
                        Text("Удалить", color = C.Err, fontSize = 15.sp, modifier = Modifier.clickable {
                            val srv = app.servers.all().firstOrNull { it.id == id }
                            scope.launch {
                                // по возможности отзываем сертификат на сервере: тогда доступ закрыт полностью, а не только на телефоне
                                if (srv != null && com.nexora.mobile.data.Gate.unlocked) try { com.nexora.mobile.net.Api(srv).revokeSelf() } catch (_: Exception) { }
                                app.servers.remove(id); app.conn.setServer(app.servers.active()); confirmRemove = null; rev++; onChange()
                            }
                        }.padding(end = 24.dp, top = 6.dp, bottom = 6.dp))
                        Text("Отмена", color = C.Muted, fontSize = 15.sp, modifier = Modifier.clickable { confirmRemove = null }.padding(vertical = 6.dp))
                    }
                }
            }

            SectionTitle("Режим действий агента")
            Column(Modifier.fillMaxWidth().panel()) {
                val st = app.chat.status
                val online = conn as? Conn.Online
                LaunchedEffect(online != null, st?.trustPending) {
                    while (online != null && st?.trustPending == true) { kotlinx.coroutines.delay(3000); app.chat.loadStatus(online.server) }
                }
                when {
                    online == null -> Item("Нет связи с сервером", "Статус режима действий появится после подключения")
                    st?.trusted == true -> {
                        Item("Включён для этого устройства", "Агент выполняет по вашим запросам обычные действия без вопросов. Удаление, права администратора и боевой сервер подтверждаются всегда, с кодом Authenticator.")
                        Item("Отключить", color = C.Err, onClick = { app.chat.revokeTrust(online.server) })
                    }
                    st?.trustPending == true -> Item("Ждёт одобрения на ПК", "Откройте программу Nexora на ПК, раздел «Сервер», и разрешите режим действий для этого телефона.")
                    else -> {
                        Item("Не включён", "Агент спрашивает подтверждение на каждое действие. Запросите режим действий один раз, владелец одобряет его в программе на ПК, дальше для этого устройства вопросы по обычным действиям не появляются.")
                        Item("Запросить режим действий", color = C.PurpleHi, onClick = { app.chat.requestTrust(online.server) })
                    }
                }
            }

            SectionTitle("Безопасность")
            Column(Modifier.fillMaxWidth().panel()) {
                Toggle("Скрывать экран в «Недавних»", "Запрещает снимки экрана и превью приложения (FLAG_SECURE)", secure) { secure = it; p.secureScreen = it; onChange() }
                Item("Соединение", "Только TLS 1.3, сервер проверяется по отпечатку и вашему CA, системные сертификаты не используются")
            }

            SectionTitle("Уведомления")
            Column(Modifier.fillMaxWidth().panel()) {
                Toggle("Задача завершена", null, nDone) { nDone = it; p.notifyDone = it }
                Toggle("Требуется подтверждение", null, nAppr) { nAppr = it; p.notifyApprove = it }
                Toggle("Ошибка выполнения", null, nErr) { nErr = it; p.notifyError = it }
                Toggle("Сервер отключился", null, nOff) { nOff = it; p.notifyOffline = it }
                Item("Приватность", "В уведомлениях нет команд, путей и данных. На заблокированном экране виден только общий заголовок.")
            }

            SectionTitle("Внешний вид")
            Column(Modifier.fillMaxWidth().panel()) {
                Toggle("Анимации робота", "Выключите, чтобы робот показывал состояния без движения", anim) { anim = it; p.animations = it; onChange() }
                Item("Язык интерфейса", "Русский (другие языки пока не добавлены)")
            }

            SectionTitle("О приложении")
            Column(Modifier.fillMaxWidth().panel()) {
                Item("Nexora Agent 0.1.0", "Мобильный пульт управления ИИ-агентом на вашем сервере. Модели и команды выполняются на сервере.")
            }
            Spacer(Modifier.size(24.dp))
        }
    }
}
