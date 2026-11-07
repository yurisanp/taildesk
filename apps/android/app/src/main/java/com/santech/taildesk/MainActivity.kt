package com.santech.taildesk

import android.app.NativeActivity
import android.content.ActivityNotFoundException
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

    /** O Rust já subiu (antes disso, o que chega espera em [compartilharPendente]). */
    @Volatile
    private var rustNoAr = false
    private var compartilharPendente: Intent? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        tailscale = TailscaleState(this)
        // A NativeActivity abre a biblioteca por conta própria, e a JVM só procura as funções
        // nativas (Ponte) em bibliotecas carregadas pelo System.loadLibrary. Carregar aqui não
        // abre outra cópia: é o mesmo arquivo, aberto uma vez pelo processo.
        System.loadLibrary("taildesk_mobile")
        super.onCreate(savedInstanceState)
        Ponte.carregada = true
        if (intent?.action == ACAO_COMPARTILHAR) compartilharPendente = intent
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.action != ACAO_COMPARTILHAR) return
        if (rustNoAr) compartilhar(intent) else compartilharPendente = intent
    }

    /** Copia os arquivos compartilhados para o cache e entrega ao Rust (MF-63). */
    private fun compartilhar(intent: Intent) {
        val clip = intent.clipData ?: return
        val uris = (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }
        if (uris.isEmpty()) return
        val app = applicationContext
        Thread { Ponte.nativeCompartilhar(SafBridge.copiarEscolhidos(app, uris)) }.start()
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
            rustNoAr = true
            compartilharPendente?.let { compartilhar(it) }
            compartilharPendente = null
        }
    }

    fun abrirTailscale() = runOnUiThread { Acoes.abrirTailscale(this) }

    fun instalarTailscale() = runOnUiThread { Acoes.instalarTailscale(this) }

    fun lerQr() = runOnUiThread { startActivity(Intent(this, QrScanActivity::class.java)) }

    fun copiar(texto: String) = runOnUiThread { Acoes.copiar(this, texto) }

    /**
     * Texto da área de transferência, para a sessão anunciar ao computador. O Android 10+ só
     * entrega com o app em foco (o Rust chama ao abrir a sessão e ao voltar à frente). Vazio =
     * nada ou sem acesso.
     */
    fun lerAreaTransferencia(): String = Acoes.lerAreaTransferencia(this)

    fun abrirLink(url: String) = runOnUiThread { Acoes.abrirLink(this, url) }

    fun otimizacaoBateria() = runOnUiThread { Acoes.otimizacaoBateria(this) }

    /** Seletor de arquivos do sistema para enviar ao computador (a resposta vai para o Rust). */
    fun escolherArquivos() = runOnUiThread {
        try {
            @Suppress("DEPRECATION") // NativeActivity não é ComponentActivity: sem ActivityResult API.
            startActivityForResult(SafBridge.intentEscolher(), SafBridge.PEDIDO_ESCOLHER)
        } catch (_: ActivityNotFoundException) {
            Ponte.nativeArquivosEscolhidos("")
        }
    }

    /** `caminho\tnome`: leva um download para `Download/TailDesk` e avisa o Rust. */
    fun publicarDownload(texto: String) {
        val (caminho, nome) = texto.split('\t', limit = 2).let { it[0] to it.getOrElse(1) { "" } }
        val app = applicationContext
        Thread {
            val ok = SafBridge.publicar(app, caminho, SafBridge.nomeSeguro(nome))
            Ponte.nativeDownloadPublicado((if (ok) "ok" else "erro") + "\t" + nome)
        }.start()
    }

    @Deprecated("NativeActivity não tem a ActivityResult API")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != SafBridge.PEDIDO_ESCOLHER) return
        val uris = if (resultCode == RESULT_OK) SafBridge.uris(data) else emptyList()
        if (uris.isEmpty()) return
        val app = applicationContext
        Thread { Ponte.nativeArquivosEscolhidos(SafBridge.copiarEscolhidos(app, uris)) }.start()
    }

    /**
     * Tela inicial pedida por quem abriu o app (testes no emulador e capturas da loja):
     * `demo` (modo demonstração), `exemplo` (lista de dispositivos de exemplo) ou `s1` (medição
     * de quadros por segundo do spike S1, docs/context/mobile/m0-resultados.md). Vazio = normal.
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
        const val ACAO_COMPARTILHAR = "com.santech.taildesk.COMPARTILHAR"
    }
}
