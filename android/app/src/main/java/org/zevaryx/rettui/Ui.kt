package org.zevaryx.rettui

import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/**
 * The colours of the app's own screens: the web UI's (its `--bg`, `--panel`
 * and so on), in the theme the page last had (see MainActivity.paint), so
 * the screens opened from it don't switch themes.
 */
class Palette(val light: Boolean, val background: Int) {
    val panel = if (light) Color.WHITE else Color.rgb(0x1c, 0x1c, 0x26)
    val border = if (light) Color.rgb(0xc8, 0xc8, 0xd4) else Color.rgb(0x3b, 0x3b, 0x4f)
    val text = if (light) Color.rgb(0x1b, 0x1b, 0x24) else Color.rgb(0xdc, 0xdc, 0xe4)
    val dim = if (light) Color.rgb(0x5d, 0x5d, 0x6e) else Color.rgb(0x9a, 0x9a, 0xae)
    val accent = if (light) Color.rgb(0x00, 0x72, 0x7c) else Color.rgb(0x4f, 0xd6, 0xe0)
    val onAccent = if (light) Color.WHITE else Color.rgb(0x0b, 0x1e, 0x21)
}

fun Context.palette(): Palette {
    val prefs = getSharedPreferences("app", Context.MODE_PRIVATE)
    val light = prefs.getBoolean("page_light", false)
    val fallback = if (light) Color.rgb(0xf3, 0xf3, 0xf6) else BACKGROUND
    return Palette(light, prefs.getInt("page_background", fallback))
}

/** The theme for this screen, the page's: before the screen is made. */
fun Activity.applyPageTheme() = setTheme(if (palette().light) R.style.Theme_Rettui_Light else R.style.Theme_Rettui)

/** Dialogs on the page's theme (rounded, on its panel colour, buttons in its accent). */
fun Context.dialog(): AlertDialog.Builder = AlertDialog.Builder(this, if (palette().light) R.style.Dialog_Light else R.style.Dialog)

private fun Context.rounded(fill: Int, stroke: Int? = null, radius: Int = 12): GradientDrawable = GradientDrawable().apply {
    setColor(fill)
    cornerRadius = dp(radius).toFloat()
    if (stroke != null) setStroke(dp(1), stroke)
}

private fun Context.ripple(background: Drawable, color: Int): Drawable =
    RippleDrawable(ColorStateList.valueOf(color and 0x00ffffff or 0x33000000), background, rounded(Color.WHITE))

/** Wide screens (tablets, landscape) keep the content to a readable column. */
private class Column(context: Context) : LinearLayout(context) {
    init {
        orientation = VERTICAL
    }

    override fun onMeasure(widthSpec: Int, heightSpec: Int) {
        val most = context.dp(640)
        val width = MeasureSpec.getSize(widthSpec)
        super.onMeasure(if (width > most) MeasureSpec.makeMeasureSpec(most, MeasureSpec.EXACTLY) else widthSpec, heightSpec)
    }
}

/**
 * A screen of the app's own: a bar with a back arrow and `title`, and below
 * it `content`, a scrolling column of [section]s. Kept clear of the bars.
 */
class Page(private val activity: Activity, title: String) {
    private val palette = activity.palette()
    val content: LinearLayout = Column(activity).apply { setPadding(activity.dp(16), activity.dp(4), activity.dp(16), activity.dp(24)) }

    init {
        activity.title = title
        val bar = LinearLayout(activity).apply {
            gravity = Gravity.CENTER_VERTICAL
            setPadding(activity.dp(4), 0, activity.dp(16), 0)
            addView(ImageButton(activity).apply {
                setImageResource(R.drawable.ic_back)
                imageTintList = ColorStateList.valueOf(palette.text)
                contentDescription = activity.getString(R.string.back)
                background = activity.ripple(GradientDrawable().apply { setColor(Color.TRANSPARENT) }, palette.text)
                setOnClickListener { activity.finish() }
            }, LinearLayout.LayoutParams(activity.dp(48), activity.dp(48)))
            addView(TextView(activity).apply {
                text = title
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
                setTypeface(typeface, Typeface.BOLD)
                setTextColor(palette.text)
                setPadding(activity.dp(4), 0, 0, 0)
            })
        }
        val scroll = ScrollView(activity).apply {
            isFillViewport = true
            addView(FrameLayout(activity).apply {
                addView(content, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER_HORIZONTAL))
            })
        }
        activity.showClearOfBars(LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            addView(bar, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, activity.dp(56)))
            addView(scroll, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        })
    }

    /** A card with `heading` over what's added to it. */
    fun section(heading: String? = null): LinearLayout {
        val card = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            background = activity.rounded(palette.panel, palette.border, 14)
            setPadding(activity.dp(16), activity.dp(14), activity.dp(16), activity.dp(14))
        }
        content.addView(card, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply {
            topMargin = activity.dp(12)
        })
        if (heading != null) card.heading(heading)
        return card
    }
}

fun LinearLayout.heading(text: String): TextView = TextView(context).apply {
    this.text = text
    setTextSize(TypedValue.COMPLEX_UNIT_SP, 17f)
    setTypeface(typeface, Typeface.BOLD)
    setTextColor(context.palette().text)
    setPadding(0, 0, 0, context.dp(6))
}.also { addView(it) }

/** Explanation: dimmer than the headings, and roomy to read. */
fun LinearLayout.note(text: String): TextView = TextView(context).apply {
    this.text = text
    setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
    setLineSpacing(0f, 1.15f)
    setTextColor(context.palette().dim)
    setPadding(0, 0, 0, context.dp(6))
}.also { addView(it) }

/** A fact or a state, in the text colour. */
fun LinearLayout.value(text: String = ""): TextView = TextView(context).apply {
    this.text = text
    setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
    setTextColor(context.palette().text)
    setPadding(0, context.dp(2), 0, context.dp(6))
}.also { addView(it) }

private fun LinearLayout.button(text: String, filled: Boolean, run: () -> Unit): TextView {
    val palette = context.palette()
    return TextView(context).apply {
        this.text = text
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
        setTypeface(typeface, Typeface.BOLD)
        gravity = Gravity.CENTER
        minHeight = context.dp(48)
        setPadding(context.dp(16), context.dp(8), context.dp(16), context.dp(8))
        setTextColor(if (filled) palette.onAccent else palette.accent)
        background = context.ripple(if (filled) context.rounded(palette.accent, null, 10) else context.rounded(Color.TRANSPARENT, palette.border, 10), palette.accent)
        isClickable = true
        isFocusable = true
        setOnClickListener { run() }
    }.also {
        addView(it, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = context.dp(8) })
    }
}

/** The thing to do on a screen: filled with the accent. */
fun LinearLayout.primary(text: String, run: () -> Unit) = button(text, true, run)

/** Any other action: outlined. */
fun LinearLayout.secondary(text: String, run: () -> Unit) = button(text, false, run)

/** A tick or radio button's colour: the accent when on, the dim colour off. */
fun android.widget.CompoundButton.tinted() {
    val palette = context.palette()
    buttonTintList = ColorStateList(
        arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()),
        intArrayOf(palette.accent, palette.dim),
    )
    setTextColor(palette.text)
}
