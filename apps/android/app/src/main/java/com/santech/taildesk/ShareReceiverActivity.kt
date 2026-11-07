package com.santech.taildesk

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle

/**
 * Alvo de "Compartilhar com TailDesk" no menu do sistema (MF-63). Não tem tela: repassa os
 * arquivos para a [MainActivity] (com a permissão de leitura de cada URI) e fecha. Lá a pessoa
 * escolhe o computador e o envio vai sem abrir a tela da sessão.
 */
class ShareReceiverActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uris = urisCompartilhadas(intent)
        if (uris.isNotEmpty()) {
            val clip = ClipData.newRawUri("", uris[0])
            uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
            val repasse = Intent(this, MainActivity::class.java).setAction(MainActivity.ACAO_COMPARTILHAR)
            // `setClipData` não devolve o Intent.
            repasse.clipData = clip
            repasse.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            startActivity(repasse)
        }
        finish()
    }

    companion object {
        /** URIs de um `ACTION_SEND` ou `ACTION_SEND_MULTIPLE`. */
        fun urisCompartilhadas(intent: Intent?): List<Uri> {
            if (intent == null) return emptyList()
            return when (intent.action) {
                Intent.ACTION_SEND -> listOfNotNull(stream(intent))
                Intent.ACTION_SEND_MULTIPLE -> streams(intent)
                else -> emptyList()
            }
        }

        private fun stream(intent: Intent): Uri? =
            if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }

        private fun streams(intent: Intent): List<Uri> =
            if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            }
    }
}
