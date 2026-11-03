package com.santech.taildesk

import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import java.net.Inet6Address
import java.net.NetworkInterface

/**
 * O que o Android diz da rede, para o Rust classificar o estado do Tailscale
 * (`taildesk_overlay::mobile::classificar`, spike S4): app instalado e versão, a rede ativa
 * deste app ser VPN, rede tarifada e os IPs das interfaces (o 100.x do Tailscale aparece mesmo
 * quando o TailDesk está fora do túnel).
 */
class TailscaleState(private val context: Context) {

    data class Sinais(
        val instalado: Boolean,
        val versao: String?,
        val redeVpn: Boolean,
        val tarifada: Boolean,
        val ips: List<String>,
    )

    private val cm = context.getSystemService(ConnectivityManager::class.java)
    private var callback: ConnectivityManager.NetworkCallback? = null

    fun ler(): Sinais {
        val versao = try {
            context.packageManager.getPackageInfo(PACOTE_TAILSCALE, 0).versionName
        } catch (_: PackageManager.NameNotFoundException) {
            null
        }
        val caps = cm?.activeNetwork?.let { cm.getNetworkCapabilities(it) }
        return Sinais(
            instalado = versao != null,
            versao = versao,
            redeVpn = caps?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true,
            tarifada = cm?.isActiveNetworkMetered == true,
            ips = ipsDasInterfaces(),
        )
    }

    /** Chama [aoMudar] a cada mudança de rede (inclusive VPN ligando e desligando). */
    fun observar(aoMudar: (Sinais) -> Unit) {
        parar()
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = aoMudar(ler())
            override fun onLost(network: Network) = aoMudar(ler())
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) = aoMudar(ler())
        }
        val pedido = NetworkRequest.Builder()
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
        cm?.registerNetworkCallback(pedido, cb)
        callback = cb
    }

    fun parar() {
        callback?.let { runCatching { cm?.unregisterNetworkCallback(it) } }
        callback = null
    }

    companion object {
        const val PACOTE_TAILSCALE = "com.tailscale.ipn"

        /** Formato da ponte: `instalado\tversão\tvpn\ttarifada\tip,ip`. */
        fun texto(s: Sinais): String = listOf(
            if (s.instalado) "1" else "0",
            s.versao.orEmpty().replace('\t', ' '),
            if (s.redeVpn) "1" else "0",
            if (s.tarifada) "1" else "0",
            s.ips.joinToString(","),
        ).joinToString("\t")

        fun ipsDasInterfaces(): List<String> = try {
            NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
                .filter { it.isUp }
                .flatMap { it.inetAddresses.toList() }
                .filter { !it.isLoopbackAddress && !it.isLinkLocalAddress }
                .map { a ->
                    val h = a.hostAddress.orEmpty()
                    if (a is Inet6Address) h.substringBefore('%') else h
                }
        } catch (_: Exception) {
            emptyList()
        }
    }
}
