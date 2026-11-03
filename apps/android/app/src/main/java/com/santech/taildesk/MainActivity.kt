package com.santech.taildesk

import android.app.NativeActivity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import java.util.Locale

/**
 * Hospeda a interface Slint (NativeActivity do backend `android-activity`; a biblioteca é
 * `libtaildesk_mobile.so`, ver o manifesto). Os métodos públicos são chamados pelo Rust por JNI
 * (`crates/taildesk-mobile/src/android.rs`): não renomeie sem mudar lá e no proguard-rules.pro.
 */
class MainActivity : NativeActivity() {

    private lateinit var tailscale: TailscaleState

    override fun onCreate(savedInstanceState: Bundle?) {
        tailscale = TailscaleState(this)
        super.onCreate(savedInstanceState)
        Ponte.carregada = true
    }

    override fun onDestroy() {
        tailscale.parar()
        super.onDestroy()
    }

    /** O Rust terminou de subir: começa a mandar o estado da rede. */
    fun rustPronto() {
        runOnUiThread {
            Ponte.nativeSinais(TailscaleState.texto(tailscale.ler()))
            tailscale.observar { Ponte.nativeSinais(TailscaleState.texto(it)) }
        }
    }

    fun abrirTailscale() = runOnUiThread { Acoes.abrirTailscale(this) }

    fun instalarTailscale() = runOnUiThread { Acoes.instalarTailscale(this) }

    fun lerQr() = runOnUiThread { startActivity(Intent(this, QrScanActivity::class.java)) }

    fun copiar(texto: String) = runOnUiThread { Acoes.copiar(this, texto) }

    fun abrirLink(url: String) = runOnUiThread { Acoes.abrirLink(this, url) }

    fun otimizacaoBateria() = runOnUiThread { Acoes.otimizacaoBateria(this) }

    /**
     * Tela inicial pedida por quem abriu o app (testes no emulador e capturas da loja):
     * `demo` (modo demonstração) ou `exemplo` (lista de dispositivos de exemplo). Vazio = normal.
     */
    fun modoInicial(): String = intent?.getStringExtra(EXTRA_MODO).orEmpty()

    /**
     * modelo, tablet (0/1), largura e altura em pixels físicos, dpi, densidade, idioma do sistema
     * e versão do app, separados por tabulação.
     */
    fun aparelho(): String {
        val m = resources.displayMetrics
        val tablet = resources.configuration.smallestScreenWidthDp >= 600
        val idioma = resources.configuration.locales.get(0) ?: Locale.getDefault()
        val versao = packageManager.getPackageInfo(packageName, 0).versionName.orEmpty()
        return listOf(
            "${Build.MANUFACTURER} ${Build.MODEL}".trim(),
            if (tablet) "1" else "0",
            maxOf(m.widthPixels, m.heightPixels).toString(),
            minOf(m.widthPixels, m.heightPixels).toString(),
            m.densityDpi.toString(),
            m.density.toString(),
            idioma.toLanguageTag(),
            versao,
        ).joinToString("\t") { it.replace('\t', ' ') }
    }

    companion object {
        const val EXTRA_MODO = "taildesk.modo"
    }
}
