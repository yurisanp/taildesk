package com.santech.taildesk

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import android.view.Gravity
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Lê o QR do desktop (mockup 03). CameraX mostra a câmera e entrega o plano Y de cada quadro; a
 * decodificação é em Rust (`rqrr`), sem ML Kit. A permissão da câmera é pedida só aqui (MNF-10).
 */
class QrScanActivity : ComponentActivity() {

    private val achou = AtomicBoolean(false)
    private val analise = Executors.newSingleThreadExecutor()
    private lateinit var previa: PreviewView

    private val pedirCamera = registerForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        if (ok) abrirCamera() else {
            Toast.makeText(this, R.string.qr_sem_camera, Toast.LENGTH_LONG).show()
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        previa = PreviewView(this)
        val textos = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 48, 48, 48)
            setBackgroundColor(0xB3000000.toInt())
            addView(TextView(context).apply {
                setText(R.string.qr_titulo)
                setTextColor(0xFFFFFFFF.toInt())
                textSize = 20f
            })
            addView(TextView(context).apply {
                setText(R.string.qr_ajuda)
                setTextColor(0xCCFFFFFF.toInt())
                textSize = 14f
            })
            addView(Button(context).apply {
                setText(R.string.qr_cancelar)
                setOnClickListener { finish() }
            })
        }
        setContentView(FrameLayout(this).apply {
            addView(previa)
            addView(textos, FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM,
            ))
        })
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            abrirCamera()
        } else {
            pedirCamera.launch(Manifest.permission.CAMERA)
        }
    }

    override fun onDestroy() {
        analise.shutdown()
        super.onDestroy()
    }

    private fun abrirCamera() {
        val futuro = ProcessCameraProvider.getInstance(this)
        futuro.addListener({
            val provedor = futuro.get()
            val preview = Preview.Builder().build().also { it.surfaceProvider = previa.surfaceProvider }
            val leitura = ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build()
            leitura.setAnalyzer(analise) { imagem ->
                imagem.use {
                    if (achou.get()) return@setAnalyzer
                    val plano = it.planes[0]
                    val buf = plano.buffer
                    val bytes = ByteArray(buf.remaining()).also { b -> buf.get(b) }
                    val uri = Ponte.nativeLerQuadro(it.width, it.height, plano.rowStride, bytes)
                    if (uri != null && achou.compareAndSet(false, true)) {
                        Ponte.nativeQr(uri)
                        runOnUiThread { finish() }
                    }
                }
            }
            provedor.unbindAll()
            provedor.bindToLifecycle(this, CameraSelector.DEFAULT_BACK_CAMERA, preview, leitura)
        }, ContextCompat.getMainExecutor(this))
    }
}
