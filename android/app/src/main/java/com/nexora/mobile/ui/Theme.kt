package com.nexora.mobile.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

object C {
    val Bg = Color(0xFF0B0817)
    val Panel = Color(0xFF16102A)
    val Panel2 = Color(0xFF1E1638)
    val Line = Color(0xFF3A2B66)
    val Purple = Color(0xFF8B5CF6)
    val PurpleHi = Color(0xFFA78BFA)
    val Violet = Color(0xFF6D3FD9)
    val Teal = Color(0xFF2DD4BF)
    val Blue = Color(0xFF60A5FA)
    val Ok = Color(0xFF34D399)
    val Warn = Color(0xFFFBBF24)
    val Err = Color(0xFFFF5C7A)
    val Text = Color(0xFFF1ECFF)
    val Muted = Color(0xFF9D93C2)
    val Btn = Brush.horizontalGradient(listOf(Color(0xFF7C4DFF), Color(0xFFA855F7)))
}

@Composable
fun NexoraTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = C.Purple, onPrimary = Color.White, background = C.Bg, surface = C.Panel, onSurface = C.Text,
            onBackground = C.Text, surfaceVariant = C.Panel2, onSurfaceVariant = C.Muted, error = C.Err, outline = C.Line,
        ),
        typography = Typography(),
        content = content,
    )
}

/** Скруглённая панель с тонкой фиолетовой рамкой. */
fun Modifier.panel(radius: Dp = 18.dp, fill: Color = C.Panel, border: Color = C.Line): Modifier =
    this.clip(RoundedCornerShape(radius)).background(fill).border(BorderStroke(1.dp, border), RoundedCornerShape(radius))
