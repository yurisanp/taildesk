package com.santech.taildesk

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.widget.Toast

/** O que a interface em Rust pede ao Android (android-detalhado.md §4.3). */
object Acoes {
    /** Abre o app do Tailscale; sem ele, a página dele na Play Store. */
    fun abrirTailscale(context: Context) {
        val abrir = context.packageManager.getLaunchIntentForPackage(TailscaleState.PACOTE_TAILSCALE)
        if (abrir != null) {
            context.startActivity(abrir.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } else {
            instalarTailscale(context)
        }
    }

    fun intentLoja(): Intent = Intent(
        Intent.ACTION_VIEW,
        Uri.parse("market://details?id=${TailscaleState.PACOTE_TAILSCALE}"),
    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    fun instalarTailscale(context: Context) {
        try {
            context.startActivity(intentLoja())
        } catch (_: ActivityNotFoundException) {
            abrirLink(context, "https://play.google.com/store/apps/details?id=${TailscaleState.PACOTE_TAILSCALE}")
        }
    }

    fun abrirLink(context: Context, url: String) {
        try {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(context, R.string.sem_navegador, Toast.LENGTH_SHORT).show()
        }
    }

    fun copiar(context: Context, texto: String) {
        val cb = context.getSystemService(ClipboardManager::class.java) ?: return
        cb.setPrimaryClip(ClipData.newPlainText("TailDesk", texto))
    }

    /**
     * Tela do sistema da otimização de bateria (sem declarar REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
     * que a Play só aceita em casos listados; android-detalhado.md §4.4).
     */
    fun otimizacaoBateria(context: Context) {
        try {
            context.startActivity(
                Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        } catch (_: ActivityNotFoundException) {
            context.startActivity(Intent(Settings.ACTION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
    }
}
