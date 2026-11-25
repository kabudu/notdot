package dev.notdot.mobile

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.util.AttributeSet
import com.journeyapps.barcodescanner.CaptureActivity
import com.journeyapps.barcodescanner.DecoratedBarcodeView
import com.journeyapps.barcodescanner.ViewfinderView

class PairingCaptureActivity : CaptureActivity() {
    override fun initializeContent(): DecoratedBarcodeView {
        setContentView(R.layout.pairing_capture)
        return findViewById(R.id.zxing_barcode_scanner)
    }
}

/** The guides use the same rectangle that the decoder crops to. */
class PairingViewfinder(context: Context, attrs: AttributeSet) : ViewfinderView(context, attrs) {
    private val density = resources.displayMetrics.density
    private val guide = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFFB4F2FF.toInt()
        strokeWidth = 3 * density
        strokeCap = Paint.Cap.ROUND
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val frame = framingRect ?: return
        val arm = 22 * density
        for (x in listOf(frame.left.toFloat(), frame.right.toFloat())) {
            val horizontal = if (x == frame.left.toFloat()) arm else -arm
            for (y in listOf(frame.top.toFloat(), frame.bottom.toFloat())) {
                val vertical = if (y == frame.top.toFloat()) arm else -arm
                canvas.drawLine(x, y, x + horizontal, y, guide)
                canvas.drawLine(x, y, x, y + vertical, guide)
            }
        }
    }
}
