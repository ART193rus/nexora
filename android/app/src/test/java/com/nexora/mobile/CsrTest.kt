package com.nexora.mobile

import com.nexora.mobile.data.Identity
import com.nexora.mobile.data.Link
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File
import java.security.KeyPairGenerator
import java.security.spec.ECGenParameterSpec

class CsrTest {
    @Test fun writesCsrForOpenssl() {
        val kp = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
        val pem = Identity.buildCsr("Pixel 8 «тест»", kp.public, kp.private)
        File(System.getProperty("csr.out") ?: "build/csr.pem").apply { parentFile.mkdirs(); writeText(pem) }
    }

    @Test fun parsesLinks() {
        val fp = "a".repeat(64)
        assertEquals(Link("192.168.1.5", 40123, fp), Link.parse("nexora://192.168.1.5:40123#$fp"))
        assertEquals(Link("::1", 443, fp), Link.parse("nexora://[::1]:443#$fp"))
        assertNull(Link.parse("https://x:1#$fp"))
        assertNull(Link.parse("nexora://host:1"))
        assertNull(Link.parse("nexora://host:1#abc"))
        assertNull(Link.parse("nexora://host:99999#$fp"))
        assertNotNull(Link.parse("nexora://my-pc.local:8443#${fp.uppercase()}"))
    }
}
