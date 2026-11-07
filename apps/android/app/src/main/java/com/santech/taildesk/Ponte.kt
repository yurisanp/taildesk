package com.santech.taildesk

/**
 * Funções nativas da biblioteca Rust (`crates/taildesk-mobile/src/android.rs`).
 * Mensagens em texto com campos separados por tabulação, para a ponte ficar pequena.
 */
object Ponte {
    /** `instalado\tversão\tvpn\ttarifada\tips` (ver [TailscaleState.texto]). */
    @JvmStatic external fun nativeSinais(texto: String)

    /** A câmera leu o QR de pareamento (URI `taildesk://pair?...`). */
    @JvmStatic external fun nativeQr(texto: String)

    /** Decodifica um quadro (plano Y) da câmera; devolve a URI ou `null`. */
    @JvmStatic external fun nativeLerQuadro(largura: Int, altura: Int, stride: Int, luma: ByteArray): String?

    /** Arquivos escolhidos no seletor, já no cache: `caminho\tnome\ttamanho` por linha. */
    @JvmStatic external fun nativeArquivosEscolhidos(texto: String)

    /** Resultado de [MainActivity.publicarDownload]: `ok\tnome` ou `erro\tnome`. */
    @JvmStatic external fun nativeDownloadPublicado(texto: String)

    /** A biblioteca é carregada pela NativeActivity; fora dela (testes JVM) não existe. */
    @Volatile
    var carregada = false
}
