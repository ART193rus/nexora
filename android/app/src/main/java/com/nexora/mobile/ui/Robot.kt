package com.nexora.mobile.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ColorMatrix
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.nexora.mobile.R
import kotlin.math.PI
import kotlin.math.sin

enum class RobotState { Idle, Connecting, Thinking, Executing, Success, Error, Offline }

// Геометрия исходной картинки корпуса 236x207 (из десктопной версии): область лица (визор).
private const val BW = 236f
private const val BH = 207f
private const val FX = 72f
private const val FY = 53f
private const val FW = 116f
private const val FH = 70f
private const val GX = 72f
private const val GY = 47f
private const val GW = 116f
private const val GH = 80f

private val GLOW = Color(0xFFE2D2EC)

/**
 * Робот NEXORA. Корпус и «спокойное» лицо взяты из оригинальных картинок, лица состояний нарисованы векторно в визоре.
 * Анимация идёт только если [animated]=true; иначе показывается неподвижная поза нужного состояния.
 */
@Composable
fun Robot(state: RobotState, animated: Boolean, modifier: Modifier = Modifier) {
    val t: State<Float> = if (animated) {
        rememberInfiniteTransition(label = "robot").animateFloat(0f, 1f, infiniteRepeatable(tween(1600, easing = LinearEasing), RepeatMode.Restart), label = "t")
    } else remember { mutableFloatStateOf(0f) }
    val slow: State<Float> = if (animated) {
        rememberInfiniteTransition(label = "robot2").animateFloat(0f, 1f, infiniteRepeatable(tween(4800, easing = LinearEasing), RepeatMode.Restart), label = "s")
    } else remember { mutableFloatStateOf(0f) }

    val bob = if (animated && state != RobotState.Error && state != RobotState.Offline) sin(slow.value * 2 * PI).toFloat() else 0f
    val gray = if (state == RobotState.Offline) ColorFilter.colorMatrix(ColorMatrix().apply { setToSaturation(0.25f) }) else null

    Box(modifier.aspectRatio(BW / BH).graphicsLayer { translationY = bob * size.height * -0.02f }) {
        Image(painterResource(R.drawable.robot_base), null, Modifier.fillMaxSize(), contentScale = ContentScale.FillBounds, colorFilter = gray)
        if (state == RobotState.Idle) {
            // оригинальное лицо; моргает раз в цикл
            val blink = if (animated) { val s = slow.value; if (s in 0.92f..0.97f) 0.08f else 1f } else 1f
            Image(
                painterResource(R.drawable.robot_glyph), null,
                Modifier.fillMaxSize().graphicsLayer {
                    // сместить и уменьшить glyph до области визора
                    scaleX = GW / BW; scaleY = (GH / BH) * blink
                    translationX = (GX + GW / 2 - BW / 2) / BW * size.width
                    translationY = (GY + GH / 2 - BH / 2) / BH * size.height
                },
                contentScale = ContentScale.FillBounds,
            )
        } else {
            Canvas(Modifier.fillMaxSize()) {
                val u = size.width / BW
                // система координат лица 68x40, центр (34,20), растянута на область лица
                val k = FW * u / 68f
                val ox = FX * u
                val oy = FY * (size.height / BH) + (FH * (size.height / BH) - 40f * k) / 2f
                placed(ox, oy, k) { drawFace(state, t.value) }
            }
        }
    }
}

private fun DrawScope.placed(x: Float, y: Float, k: Float, block: DrawScope.() -> Unit) {
    withTransform({ translate(x, y); scale(k, k, Offset.Zero) }) { block() }
}

private fun DrawScope.eye(c: Offset, r: Float, color: Color) = drawCircle(color, r, c)

private fun DrawScope.stroke(color: Color, w: Float = 4.4f) = Stroke(w, cap = StrokeCap.Round)

private fun DrawScope.drawFace(state: RobotState, t: Float) {
    val ph = (t * 2 * PI).toFloat()
    when (state) {
        RobotState.Connecting -> {
            // вращающееся кольцо из точек
            rotate(t * 360f, Offset(34f, 20f)) {
                drawCircle(GLOW, 10f, Offset(34f, 20f), style = Stroke(4.6f, cap = StrokeCap.Round, pathEffect = PathEffect.dashPathEffect(floatArrayOf(1.4f, 6.6f))))
            }
            drawCircle(GLOW, 2f, Offset(34f, 20f))
        }
        RobotState.Thinking -> {
            val dx = sin(ph) * 4f
            eye(Offset(24f + dx, 14f), 4.4f, GLOW); eye(Offset(44f + dx, 14f), 4.4f, GLOW)
            for (i in 0..2) {
                val a = (0.35f + 0.65f * ((sin(ph - i * 1.2f) + 1f) / 2f))
                drawCircle(GLOW.copy(alpha = a), 2.4f, Offset(24f + i * 10f, 29f))
            }
        }
        RobotState.Executing -> {
            // глаза-щели и бегущий по визору индикатор
            drawLine(GLOW, Offset(18f, 14f), Offset(28f, 14f), 4.4f, StrokeCap.Round)
            drawLine(GLOW, Offset(40f, 14f), Offset(50f, 14f), 4.4f, StrokeCap.Round)
            val p = (sin(ph) + 1f) / 2f
            drawLine(GLOW.copy(alpha = 0.25f), Offset(10f, 30f), Offset(58f, 30f), 3f, StrokeCap.Round)
            val cx = 10f + 48f * p
            drawLine(GLOW, Offset(cx - 6f, 30f), Offset(cx + 6f, 30f), 3.6f, StrokeCap.Round)
        }
        RobotState.Success -> {
            val green = Color(0xFF7CF2C0)
            for (cx in floatArrayOf(22f, 46f)) {
                val p = Path().apply { moveTo(cx - 7f, 18f); quadraticTo(cx, 6f, cx + 7f, 18f) }
                drawPath(p, green, style = stroke(green))
            }
            val s = Path().apply { moveTo(26f, 26f); quadraticTo(34f, 34f, 42f, 26f) }
            drawPath(s, green, style = stroke(green, 3.2f))
        }
        RobotState.Error -> {
            val red = Color(0xFFFF5C7A)
            for (cx in floatArrayOf(22f, 46f)) {
                drawLine(red, Offset(cx - 5f, 10f), Offset(cx + 5f, 22f), 4.4f, StrokeCap.Round)
                drawLine(red, Offset(cx + 5f, 10f), Offset(cx - 5f, 22f), 4.4f, StrokeCap.Round)
            }
            val m = Path().apply { moveTo(27f, 33f); quadraticTo(34f, 27f, 41f, 33f) }
            drawPath(m, red, style = stroke(red, 3.2f))
        }
        RobotState.Offline -> {
            val dim = Color(0xFF8D88AD)
            drawLine(dim, Offset(16f, 16f), Offset(28f, 16f), 4.4f, StrokeCap.Round)
            drawLine(dim, Offset(40f, 16f), Offset(52f, 16f), 4.4f, StrokeCap.Round)
        }
        RobotState.Idle -> Unit
    }
}
