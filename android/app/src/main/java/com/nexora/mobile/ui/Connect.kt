package com.nexora.mobile.ui

import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.clickable
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowBack
import androidx.compose.material.icons.filled.ContentPaste
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.nexora.mobile.NexoraApp
import com.nexora.mobile.data.Identity
import com.nexora.mobile.data.Link
import com.nexora.mobile.data.Server
import com.nexora.mobile.net.Api
import com.nexora.mobile.net.Conn
import com.nexora.mobile.net.NetError
import kotlinx.coroutines.launch

/** Понятное сообщение об ошибке привязки. Сервер намеренно не говорит, что именно не так, поэтому формулировки осторожные. */
fun enrollMessage(e: NetError): String = when {
    e is NetError.Http && e.code == 403 -> {
        val skew = runCatching { (org.json.JSONObject(e.body).getLong("time") - System.currentTimeMillis() / 1000) }.getOrNull()
        if (skew != null && kotlin.math.abs(skew) > 25) "Часы ПК ${if (skew < 0) "отстают от" else "спешат относительно"} телефона на ${kotlin.math.abs(skew) / 60} мин ${kotlin.math.abs(skew) % 60} с. Синхронизируйте время на ПК и повторите."
        else "Код не подошёл. Введите свежий код из Authenticator (код меняется каждые 30 секунд)."
    }
    e is NetError.Http && e.code == 429 -> "Слишком много неверных попыток. Подождите 15 минут."
    e is NetError.Rejected -> "Сервер не принял запрос. Откройте на сервере окно привязки (действует 10 минут) и попробуйте снова."
    e is NetError.IdentityChanged -> "Отпечаток сервера не совпал со строкой подключения. Возможна подмена: привязка отменена."
    else -> e.message ?: "Ошибка подключения"
}

@Composable
fun ConnectScreen(app: NexoraApp, conn: Conn, onBack: () -> Unit, onDone: () -> Unit) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var link by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var device by remember { mutableStateOf(Build.MODEL ?: "Android") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    Screen("Подключение", actions = {}) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            Row(Modifier.clickable(onClick = onBack).padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Filled.ArrowBack, null, tint = C.Muted, modifier = Modifier.size(18.dp)); Spacer(Modifier.size(6.dp)); Text("Назад", color = C.Muted, fontSize = 14.sp)
            }
            Column(Modifier.fillMaxWidth().panel().padding(14.dp)) {
                Text("Как привязать устройство", color = C.Text, fontWeight = FontWeight600, fontSize = 15.sp)
                Spacer(Modifier.height(6.dp))
                Text(
                    "1. На сервере (Nexora Server) откройте окно привязки и скопируйте строку подключения.\n" +
                        "2. Вставьте её ниже и введите 6-значный код из Google Authenticator.\n" +
                        "Ключ создаётся в защищённом хранилище телефона и не покидает его. Секрет Authenticator приложение не хранит.",
                    color = C.Muted, fontSize = 13.sp, lineHeight = 19.sp,
                )
            }
            Spacer(Modifier.height(16.dp))
            Field(
                "Строка подключения", link, { link = it.trim(); error = null }, placeholder = "nexora://адрес:порт#отпечаток",
                trailing = {
                    Icon(Icons.Filled.ContentPaste, "Вставить", tint = C.PurpleHi, modifier = Modifier.size(22.dp).clickable {
                        val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                        cm.primaryClip?.getItemAt(0)?.text?.toString()?.let { link = it.trim() }
                    })
                },
            )
            Spacer(Modifier.height(8.dp))
            Row(Modifier.clickable {
                val opts = com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions.Builder().setBarcodeFormats(com.google.mlkit.vision.barcode.common.Barcode.FORMAT_QR_CODE).build()
                com.google.mlkit.vision.codescanner.GmsBarcodeScanning.getClient(ctx, opts).startScan()
                    .addOnSuccessListener { b -> b.rawValue?.let { link = it.trim(); error = null } }
                    .addOnFailureListener { e -> if (e !is com.google.android.gms.common.api.ApiException || e.statusCode != 16) error = "Сканер QR недоступен: вставьте строку вручную" }
            }.padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Filled.QrCodeScanner, null, tint = C.PurpleHi, modifier = Modifier.size(22.dp)); Spacer(Modifier.size(8.dp))
                Text("Сканировать QR с экрана ПК", color = C.PurpleHi, fontSize = 14.sp)
            }
            Link.parse(link)?.let { l ->
                Column(Modifier.fillMaxWidth().padding(top = 8.dp).panel(12.dp, fill = C.Panel2).padding(12.dp)) {
                    Text("Проверьте сервер перед вводом кода", color = C.Muted, fontSize = 12.sp)
                    Text("${l.host}:${l.port}", color = C.Text, fontSize = 15.sp)
                    Text("Отпечаток: " + l.fingerprint.take(16).chunked(4).joinToString("-"), color = C.Text, fontSize = 13.sp)
                    Text("Он должен совпадать с отпечатком в окне привязки на ПК.", color = C.Muted, fontSize = 12.sp)
                }
            }
            Spacer(Modifier.height(14.dp))
            Field("Код Google Authenticator", code, { code = it.filter(Char::isDigit).take(6); error = null }, placeholder = "6 цифр", number = true)
            Spacer(Modifier.height(14.dp))
            Field("Имя этого устройства", device, { device = it.take(64) })
            Spacer(Modifier.height(18.dp))
            error?.let { Text(it, color = C.Err, fontSize = 14.sp, modifier = Modifier.padding(bottom = 10.dp)) }
            val parsed = Link.parse(link)
            PrimaryButton(if (busy) "Подключаюсь…" else "Подключиться", Icons.Filled.Link, enabled = !busy && parsed != null && code.length == 6 && device.isNotBlank()) {
                val l = parsed ?: return@PrimaryButton
                val act = ctx as android.app.Activity
                if (!com.nexora.mobile.data.Gate.available(act)) { error = "Включите блокировку экрана (PIN, пароль или отпечаток): без неё ключ устройства защитить нельзя."; return@PrimaryButton }
                busy = true; error = null
                com.nexora.mobile.data.Gate.prompt(act, "Подтвердите создание ключа") { okAuth ->
                  if (!okAuth) { busy = false; error = "Подтверждение отменено"; return@prompt }
                  scope.launch {
                    val srv = Server(Server.newId(), "", l.host, l.port, l.fingerprint)
                    try {
                        val pub = Identity.create(srv.id)
                        val csr = Identity.csrPem(srv.id, device.trim(), pub)
                        val r = Api.enroll(srv, code, device.trim(), csr)
                        val enrolled = srv.copy(name = l.host, caPem = r.caPem, certPem = r.certPem, serial = r.serial)
                        app.servers.upsert(enrolled)
                        app.conn.setServer(enrolled)
                        code = ""; link = ""
                        onDone()
                    } catch (e: NetError) {
                        Identity.delete(srv.id)
                        error = enrollMessage(e)
                    } catch (e: Exception) {
                        Identity.delete(srv.id)
                        error = "Не удалось создать ключ устройства: ${e.message}"
                    } finally { busy = false }
                  }
                }
            }
            Spacer(Modifier.height(14.dp))
            if (conn is Conn.Online) {
                Row(Modifier.fillMaxWidth().panel(14.dp, fill = C.Ok.copy(alpha = 0.12f), border = C.Ok.copy(alpha = 0.5f)).padding(14.dp), horizontalArrangement = Arrangement.Center) {
                    Text("✓ Соединение готово · ${conn.server.label}", color = C.Ok, fontSize = 15.sp)
                }
            }
            val saved = app.servers.all().filter { it.enrolled }
            if (saved.isNotEmpty()) {
                SectionTitle("Сохранённые серверы")
                saved.forEach { s ->
                    Row(Modifier.fillMaxWidth().padding(bottom = 8.dp).panel(14.dp).clickable { app.servers.setActive(s.id); app.conn.setServer(s); onDone() }.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(s.name.ifBlank { s.host }, color = C.Text, fontSize = 15.sp)
                            Text(s.label, color = C.Muted, fontSize = 13.sp)
                        }
                        if (s.id == app.servers.activeId) Text("активен", color = C.Ok, fontSize = 13.sp)
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

private val FontWeight600 = androidx.compose.ui.text.font.FontWeight.SemiBold
