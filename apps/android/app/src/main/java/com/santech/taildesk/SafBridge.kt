package com.santech.taildesk

import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import java.io.File

/**
 * Arquivos da sessão (MF-61, android-detalhado.md §4.3):
 * - enviar: o seletor do sistema (SAF, `ACTION_OPEN_DOCUMENT`) e a cópia de cada escolhido para o
 *   cache do app, porque o Rust lê arquivos por caminho (retomada e BLAKE3 precisam reler);
 * - receber: o download chega na pasta privada do app e vai para `Download/TailDesk` pelo
 *   MediaStore (sem permissão de armazenamento no Android 10+).
 */
object SafBridge {
    const val PEDIDO_ESCOLHER = 41
    const val PASTA_PUBLICA = "TailDesk"
    private const val TAG = "taildesk"

    fun intentEscolher(): Intent = Intent(Intent.ACTION_OPEN_DOCUMENT)
        .addCategory(Intent.CATEGORY_OPENABLE)
        .setType("*/*")
        .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)

    /** URIs do resultado do seletor (um ou vários). */
    fun uris(data: Intent?): List<Uri> {
        if (data == null) return emptyList()
        val clip = data.clipData
        if (clip != null) return (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }
        return listOfNotNull(data.data)
    }

    /** Nome que vira o último trecho do caminho no host: sem barras, sem `.`/`..`, não vazio. */
    fun nomeSeguro(nome: String?): String {
        val n = nome.orEmpty().replace('/', '_').replace('\\', '_').replace('\t', ' ').replace('\n', ' ').trim()
        return if (n.isEmpty() || n == "." || n == "..") "arquivo" else n
    }

    /** Uma linha para `Ponte.nativeArquivosEscolhidos`. */
    fun linha(caminho: String, nome: String, tamanho: Long): String = "$caminho\t$nome\t$tamanho"

    /**
     * Copia os escolhidos para `cache/enviar/<hora>/` (rode fora da thread da interface).
     * Devolve as linhas `caminho\tnome\ttamanho`; o Rust apaga as cópias quando o envio acaba.
     */
    fun copiarEscolhidos(context: Context, uris: List<Uri>): String {
        val pasta = File(context.cacheDir, "enviar/${System.nanoTime()}")
        if (!pasta.mkdirs() && !pasta.isDirectory) return ""
        val linhas = mutableListOf<String>()
        for (uri in uris) {
            val nome = nomeSeguro(nomeDe(context, uri))
            var destino = File(pasta, nome)
            var n = 1
            while (destino.exists()) {
                destino = File(pasta, "${n++}-$nome")
            }
            try {
                val entrada = context.contentResolver.openInputStream(uri) ?: continue
                val tamanho = entrada.use { e -> destino.outputStream().use { s -> e.copyTo(s) } }
                linhas += linha(destino.absolutePath, nome, tamanho)
            } catch (e: Exception) {
                Log.w(TAG, "não foi possível ler um arquivo escolhido: ${e.javaClass.simpleName}")
                destino.delete()
            }
        }
        return linhas.joinToString("\n")
    }

    private fun nomeDe(context: Context, uri: Uri): String? = try {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
    } catch (_: Exception) {
        null
    } ?: uri.lastPathSegment

    /**
     * Move `caminho` para `Download/TailDesk/nome` (o MediaStore dá outro nome se já existir:
     * nunca sobrescreve). Rode fora da thread da interface. `true` = foi; o original é apagado.
     */
    fun publicar(context: Context, caminho: String, nome: String): Boolean {
        val origem = File(caminho)
        val valores = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, nome)
            put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/$PASTA_PUBLICA")
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val resolver = context.contentResolver
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, valores) ?: return false
        return try {
            resolver.openOutputStream(uri)?.use { s -> origem.inputStream().use { it.copyTo(s) } }
                ?: throw IllegalStateException("sem saída")
            resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
            origem.delete()
            true
        } catch (e: Exception) {
            Log.w(TAG, "não foi possível salvar em Download/$PASTA_PUBLICA: ${e.javaClass.simpleName}")
            resolver.delete(uri, null, null)
            false
        }
    }
}
