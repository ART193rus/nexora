package com.nexora.mobile.data

import android.app.Activity
import android.app.KeyguardManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.setValue

/**
 * Замок ключа устройства. Ключ в Keystore создан с требованием подтверждения пользователем: биометрия (сильная) или
 * PIN/пароль экрана. После успешного подтверждения ключ работает [WINDOW_SEC] секунд, потом нужно подтвердить снова.
 * Без разблокировки телефон не может подписать TLS-рукопожатие, поэтому сетевые запросы не отправляются.
 */
object Gate {
    const val WINDOW_SEC = 1800
    private const val MARGIN_SEC = 120

    private var until by mutableLongStateOf(0L)
    private var prompting = false

    val unlocked: Boolean get() = System.currentTimeMillis() < until

    /** Можно ли защитить ключ на этом телефоне (есть экран блокировки или биометрия). */
    fun available(a: Activity): Boolean {
        return try {
            if (Build.VERSION.SDK_INT >= 30) {
                val bm = a.getSystemService(BiometricManager::class.java) ?: return false
                bm.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL) == BiometricManager.BIOMETRIC_SUCCESS
            } else {
                a.getSystemService(KeyguardManager::class.java)?.isDeviceSecure == true
            }
        } catch (_: SecurityException) {
            a.getSystemService(KeyguardManager::class.java)?.isDeviceSecure == true
        }
    }

    fun prompt(a: Activity, title: String = "Разблокировать Nexora", onResult: (Boolean) -> Unit) {
        if (unlocked) { onResult(true); return }
        if (prompting) { onResult(false); return }
        prompting = true
        val b = BiometricPrompt.Builder(a).setTitle(title).setSubtitle("Ключ доступа к серверу защищён подтверждением")
        if (Build.VERSION.SDK_INT >= 30) {
            b.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL)
        } else {
            @Suppress("DEPRECATION") b.setDeviceCredentialAllowed(true)
        }
        try { b.build() } catch (_: Exception) { prompting = false; onResult(false); return }
        b.build().authenticate(CancellationSignal(), a.mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult?) {
                prompting = false
                until = System.currentTimeMillis() + (WINDOW_SEC - MARGIN_SEC) * 1000L
                onResult(true)
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence?) { prompting = false; onResult(false) }
        })
    }
}
