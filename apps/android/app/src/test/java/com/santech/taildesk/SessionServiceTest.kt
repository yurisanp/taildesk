package com.santech.taildesk

import android.app.Notification
import android.content.Intent
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** Português é o padrão do app (values/); inglês em values-en. */
@RunWith(RobolectricTestRunner::class)
@Config(qualifiers = "pt-rBR")
class SessionServiceTest {

    @Test
    fun notificacaoMostraOComputadorEEncerrar() {
        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        val i = Intent(ctx, SessionService::class.java).putExtra(SessionService.EXTRA_NOME, "pc-mae")
        val servico = Robolectric.buildService(SessionService::class.java, i).create().startCommand(0, 1).get()
        val n = shadowOf(servico).lastForegroundNotification
        assertNotNull("serviço em primeiro plano com notificação", n)
        assertEquals("Ouvindo pc-mae", n!!.extras.getString(Notification.EXTRA_TITLE))
        assertEquals(1, n.actions.size)
        assertEquals("Encerrar", n.actions[0].title.toString())
        assertTrue((n.flags and Notification.FLAG_ONGOING_EVENT) != 0)
    }

    @Test
    fun encerrarParaOServico() {
        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        val i = Intent(ctx, SessionService::class.java).setAction(SessionService.ACAO_ENCERRAR)
        val servico = Robolectric.buildService(SessionService::class.java, i).create().startCommand(0, 1).get()
        assertTrue(shadowOf(servico).isStoppedBySelf)
    }
}
