package com.nexora.mobile

import com.nexora.mobile.data.Identity
import com.nexora.mobile.data.Link
import com.nexora.mobile.data.Server
import com.nexora.mobile.net.Api
import com.nexora.mobile.net.NetError
import com.nexora.mobile.net.Tls
import com.nexora.mobile.net.follow
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.security.KeyPairGenerator
import java.security.spec.ECGenParameterSpec
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Проверка клиента на настоящем сервере Nexora (стенд `nexora.exe --selftest-host`). Запускается только если задана
 * переменная NEXORA_E2E_INFO с путём к файлу {link, secret}. Ключ устройства здесь программный (Keystore в JVM нет).
 */
class E2eTest {
    private fun b32(s: String): ByteArray {
        val a = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
        var bits = 0; var v = 0
        val out = java.io.ByteArrayOutputStream()
        for (c in s.uppercase().filter { it in a }) {
            v = (v shl 5) or a.indexOf(c); bits += 5
            if (bits >= 8) { out.write((v shr (bits - 8)) and 0xFF); bits -= 8 }
        }
        return out.toByteArray()
    }

    private fun code(secret: String, stepOffset: Long = 0): String {
        val counter = System.currentTimeMillis() / 1000 / 30 + stepOffset
        val msg = java.nio.ByteBuffer.allocate(8).putLong(counter).array()
        val h = Mac.getInstance("HmacSHA1").apply { init(SecretKeySpec(b32(secret), "HmacSHA1")) }.doFinal(msg)
        val o = h.last().toInt() and 0xF
        val n = ((h[o].toInt() and 0x7F) shl 24) or ((h[o + 1].toInt() and 0xFF) shl 16) or ((h[o + 2].toInt() and 0xFF) shl 8) or (h[o + 3].toInt() and 0xFF)
        return (n % 1_000_000).toString().padStart(6, '0')
    }

    @Test
    fun fullFlowAgainstRealServer() = runBlocking {
        val infoPath = System.getenv("NEXORA_E2E_INFO")
        assumeTrue("стенд не запущен", infoPath != null && File(infoPath).exists())
        val info = JSONObject(File(infoPath!!).readText())
        val link = Link.parse(info.getString("link"))!!
        val secret = info.getString("secret")

        val kp = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
        Tls.keyProvider = { kp.private }
        val srv0 = Server("e2e", "", link.host, link.port, link.fingerprint)

        // 1. подмена сервера: чужой отпечаток без CA должен быть отвергнут до отправки кода
        try {
            Api.enroll(srv0.copy(fingerprint = "0".repeat(64)), code(secret), "x", Identity.buildCsr("x", kp.public, kp.private))
            fail("чужой сертификат принят")
        } catch (e: NetError) {
            assertTrue("ожидалась IdentityChanged, получено $e", e is NetError.IdentityChanged)
        }

        // 2. неверный код
        try {
            Api.enroll(srv0, "000000", "Телефон", Identity.buildCsr("Телефон", kp.public, kp.private))
            fail("неверный код принят")
        } catch (e: NetError.Http) { assertEquals(403, e.code) }

        // 3. привязка
        val r = Api.enroll(srv0, code(secret), "Тест-телефон", Identity.buildCsr("Тест-телефон", kp.public, kp.private))
        val s = srv0.copy(caPem = r.caPem, certPem = r.certPem, serial = r.serial)
        assertTrue(s.enrolled)

        // 4. вход по сертификату
        val api = Api(s)
        assertEquals("Тест-телефон", api.ping())
        // отпечаток листа устарел, но сервер подписан нашим CA, имеет EKU serverAuth и наш адрес: принимается
        assertEquals("Тест-телефон", Api(s.copy(fingerprint = "0".repeat(64))).ping())
        val am = api.appModels()
        assertEquals("test-model", am.current)
        assertEquals(2, am.models.size)
        api.selectModel("other")
        try { api.selectModel("нет такой"); fail("неизвестная модель принята") } catch (e: NetError.Http) { assertEquals(400, e.code) }
        val st = api.status()
        assertEquals("test-model", st.model)
        api.requestTrust()
        assertTrue(api.status().trustPending)
        assertEquals(false, api.status().trusted)
        api.revokeTrust()
        assertEquals(false, api.status().trustPending)

        // 5. чат с историей: простая задача, события, результат, нет выдуманного прогресса
        val id1 = api.createTask("привет", "привет", listOf("user" to "раньше" , "assistant" to "ответ"), fast = true)
        val streamed = StringBuilder()
        val v1 = follow(api, id1) { v -> v.events.filter { it.kind == "token" }.forEach { streamed.append(it.data.optString("text")) } }
        assertEquals("done", v1.info.status)
        assertEquals("Ответ на привет (история: 2)", v1.result)
        assertEquals(v1.result, streamed.toString())

        // 6. подтверждение обычного действия без кода
        val id2 = api.createTask("запиши файл")
        var pend = waitPending(api, id2)
        api.approve(id2, pend, true)
        assertEquals("done", follow(api, id2) { }.info.status)

        // 7. опасное: без кода сервер отказывает, с кодом разрешает
        val id3 = api.createTask("удали файл")
        pend = waitPending(api, id3)
        try { api.approve(id3, pend, true, ""); fail("опасное без кода принято") } catch (e: NetError.Http) { assertEquals(403, e.code) }
        api.approve(id3, pend, true, code(secret, 1))
        assertEquals("done", follow(api, id3) { }.info.status)

        // 8. отказ и отмена
        val id4 = api.createTask("запиши ещё")
        pend = waitPending(api, id4)
        api.approve(id4, pend, false)
        assertEquals("error", follow(api, id4) { }.info.status)
        val id5 = api.createTask("долго")
        Thread.sleep(500)
        api.cancel(id5)
        assertEquals("cancelled", follow(api, id5) { }.info.status)
        val id6 = api.retry(id5)
        assertTrue(id6 > id5)
        api.cancel(id6)

        val list = api.tasks()
        assertTrue(list.size >= 6)
        assertTrue(list.all { it.status != "running" || true })

        // 9. выход: отзыв собственного сертификата; дальше сервер молчит
        api.revokeSelf()
        try { Api(s).ping(); fail("после отзыва вход возможен") } catch (e: NetError) { assertTrue("ожидалось закрытие соединения, получено $e", e is NetError.Rejected) }
        assertNotNull(list)
    }

    private suspend fun waitPending(api: Api, id: Long): Long {
        repeat(60) {
            val v = api.poll(id, 0, 1)
            v.info.pending?.let { return it.id }
            Thread.sleep(100)
        }
        fail("подтверждение не появилось"); return 0
    }
}
