package com.santech.taildesk

import android.content.Context
import android.content.Intent
import android.content.pm.PackageInfo
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.shadows.ShadowNetworkCapabilities

@RunWith(RobolectricTestRunner::class)
class TailscaleStateTest {

    private val context: Context = ApplicationProvider.getApplicationContext()

    private fun instalarTailscale(versao: String) {
        val info = PackageInfo().apply {
            packageName = TailscaleState.PACOTE_TAILSCALE
            versionName = versao
        }
        shadowOf(context.packageManager).installPackage(info)
    }

    @Test
    fun semTailscaleInstalado() {
        val s = TailscaleState(context).ler()
        assertFalse(s.instalado)
        assertEquals(null, s.versao)
        assertTrue(TailscaleState.texto(s).startsWith("0\t\t"))
    }

    @Test
    fun instaladoComVpnAtiva() {
        instalarTailscale("1.104.1")
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val caps = ShadowNetworkCapabilities.newInstance()
        shadowOf(caps).addTransportType(NetworkCapabilities.TRANSPORT_VPN)
        shadowOf(cm).setNetworkCapabilities(cm.activeNetwork, caps)
        val s = TailscaleState(context).ler()
        assertTrue(s.instalado)
        assertEquals("1.104.1", s.versao)
        assertTrue(s.redeVpn)
        val campos = TailscaleState.texto(s).split('\t')
        assertEquals(listOf("1", "1.104.1", "1"), campos.take(3))
        assertEquals(5, campos.size)
    }

    @Test
    fun textoDaPonte() {
        val t = TailscaleState.texto(
            TailscaleState.Sinais(
                instalado = true,
                versao = "1.104\t1",
                redeVpn = false,
                tarifada = true,
                ips = listOf("192.168.0.10", "100.64.0.9"),
            ),
        )
        assertEquals("1\t1.104 1\t0\t1\t192.168.0.10,100.64.0.9", t)
    }

    @Test
    fun abrirTailscaleSemOAppVaiParaALoja() {
        Acoes.abrirTailscale(context)
        val aberta: Intent? = shadowOf(context as android.app.Application).nextStartedActivity
        assertNotNull(aberta)
        assertEquals(Intent.ACTION_VIEW, aberta!!.action)
        assertTrue(aberta.dataString!!.contains("id=com.tailscale.ipn"))
    }

    @Test
    fun copiarVaiParaAAreaDeTransferencia() {
        Acoes.copiar(context, "desktop-casa.tail1234.ts.net")
        val cb = context.getSystemService(android.content.ClipboardManager::class.java)
        assertEquals("desktop-casa.tail1234.ts.net", cb.primaryClip!!.getItemAt(0).text.toString())
    }
}
