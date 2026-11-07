package com.nexora.mobile.net

import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
import kotlin.coroutines.coroutineContext

/**
 * Следит за задачей, пока она не завершится: длинный опрос с продолжением с последнего номера события.
 * Потеря сети не обрывает слежение: задача идёт на сервере, а здесь опрос повторяется с паузой.
 * Если связи нет дольше [giveUpSec], бросает последнюю ошибку (задача при этом продолжает выполняться на сервере).
 */
suspend fun follow(
    api: Api,
    id: Long,
    startAfter: Long = 0,
    giveUpSec: Int = 300,
    onView: (TaskView) -> Unit,
): TaskView {
    var after = startAfter
    var failingSince = 0L
    var pause = 1000L
    while (true) {
        coroutineContext.ensureActive()
        try {
            val v = api.poll(id, after, 20)
            failingSince = 0; pause = 1000
            after = v.lastSeq
            onView(v)
            if (!v.info.active) return v
        } catch (e: NetError) {
            when {
                e is NetError.Http && e.code == 404 -> throw e // задачи больше нет на сервере
                e is NetError.IdentityChanged -> throw e
                else -> {
                    val now = System.currentTimeMillis()
                    if (failingSince == 0L) failingSince = now
                    if (now - failingSince > giveUpSec * 1000L) throw e
                    delay(pause)
                    pause = (pause * 2).coerceAtMost(8000)
                }
            }
        }
    }
}
