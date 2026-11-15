package app.notia.mobile

import androidx.compose.foundation.text.selection.LocalTextSelectionColors
import androidx.compose.foundation.text.selection.TextSelectionColors
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private val Ink = Color(0xFF222222)
private val Paper = Color(0xFFFFFFFF)
private val Sidebar = Color(0xFFF6F6F6)
private val Muted = Color(0xFF707070)
private val Line = Color(0xFFE3E3E3)
private val Violet = Color(0xFF7B6CD9)
private val Danger = Color(0xFFE93147)

private val InkDark = Color(0xFFDADADA)
private val PaperDark = Color(0xFF1E1E1E)
private val SidebarDark = Color(0xFF262626)
private val MutedDark = Color(0xFF9E9E9E)
private val LineDark = Color(0xFF333333)
private val VioletDark = Color(0xFFA594F9)
private val DangerDark = Color(0xFFFB464C)

private val LightColors = lightColorScheme(
    background = Paper,
    onBackground = Ink,
    surface = Sidebar,
    onSurface = Ink,
    surfaceVariant = Color(0xFFF3F3F3),
    onSurfaceVariant = Muted,
    primary = Violet,
    onPrimary = Paper,
    outline = Line,
    error = Danger,
    onError = Paper,
)

private val DarkColors = darkColorScheme(
    background = PaperDark,
    onBackground = InkDark,
    surface = SidebarDark,
    onSurface = InkDark,
    surfaceVariant = Color(0xFF2A2A2A),
    onSurfaceVariant = MutedDark,
    primary = VioletDark,
    onPrimary = PaperDark,
    outline = LineDark,
    error = DangerDark,
    onError = PaperDark,
)

private val NotiaShapes = Shapes(
    extraSmall = RoundedCornerShape(6.dp),
    small = RoundedCornerShape(6.dp),
    medium = RoundedCornerShape(8.dp),
    large = RoundedCornerShape(8.dp),
    extraLarge = RoundedCornerShape(8.dp),
)

private val NotiaType = Typography(
    titleLarge = TextStyle(fontSize = 20.sp, lineHeight = 26.sp, fontWeight = FontWeight.SemiBold),
    titleMedium = TextStyle(fontSize = 16.sp, lineHeight = 22.sp, fontWeight = FontWeight.SemiBold),
    bodyLarge = TextStyle(fontSize = 17.sp, lineHeight = 26.sp, fontWeight = FontWeight.Normal),
    bodyMedium = TextStyle(fontSize = 15.sp, lineHeight = 22.sp, fontWeight = FontWeight.Normal),
    bodySmall = TextStyle(fontSize = 13.sp, lineHeight = 18.sp, fontWeight = FontWeight.Normal),
    labelLarge = TextStyle(fontSize = 14.sp, lineHeight = 18.sp, fontWeight = FontWeight.Medium),
)

@Composable
internal fun NotiaTheme(content: @Composable () -> Unit) {
    val dark = androidx.compose.foundation.isSystemInDarkTheme()
    val colors = if (dark) DarkColors else LightColors
    MaterialTheme(colorScheme = colors, typography = NotiaType, shapes = NotiaShapes) {
        CompositionLocalProvider(
            LocalTextSelectionColors provides TextSelectionColors(
                handleColor = colors.primary,
                backgroundColor = colors.primary.copy(alpha = 0.28f),
            ),
            content = content,
        )
    }
}
