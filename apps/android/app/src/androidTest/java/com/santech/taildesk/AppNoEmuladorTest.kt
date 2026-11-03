package com.santech.taildesk

import android.content.Intent
import android.graphics.Bitmap
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * Ponta a ponta no emulador (API 30 e 34 no CI): o app abre com a biblioteca Rust, mostra a
 * lista de exemplo e a sessão do modo demonstração, e não cai. As capturas ficam em
 * `files/capturas` (o workflow puxa com `adb pull`).
 */
@RunWith(AndroidJUnit4::class)
class AppNoEmuladorTest {

    private val instr = InstrumentationRegistry.getInstrumentation()
    private val device = UiDevice.getInstance(instr)

    /**
     * Abre o app e espera a Activity chegar a RESUMED. A NativeActivity não tem Views, então a
     * árvore de acessibilidade do UiAutomator nem sempre mostra a janela (API 34); o
     * ActivityScenario espera pelo ciclo de vida.
     */
    private fun abrir(modo: String): ActivityScenario<MainActivity> {
        val intent = Intent(instr.targetContext, MainActivity::class.java)
            .putExtra(MainActivity.EXTRA_MODO, modo)
        return ActivityScenario.launch(intent)
    }

    private fun vivo(cena: ActivityScenario<MainActivity>) {
        assertEquals("o app caiu", Lifecycle.State.RESUMED, cena.state)
    }

    /**
     * Espera o Slint desenhar (no emulador sem GPU, a primeira tela depois de subir a biblioteca
     * pode levar vários segundos) e guarda a captura.
     */
    private fun capturar(nome: String) {
        val pasta = File(instr.targetContext.getExternalFilesDir(null), "capturas").apply { mkdirs() }
        val arquivo = File(pasta, "$nome.png")
        val limite = System.currentTimeMillis() + 30_000
        var desenhou = false
        while (!desenhou && System.currentTimeMillis() < limite) {
            Thread.sleep(1_000)
            assertTrue("captura $nome", device.takeScreenshot(arquivo))
            val bmp = android.graphics.BitmapFactory.decodeFile(arquivo.path)
            assertNotNull(bmp)
            desenhou = temCores(bmp)
        }
        assertTrue("tela vazia em $nome", desenhou)
    }

    /** Uma tela desenhada tem mais de uma cor (tela preta ou branca = o Slint não desenhou). */
    private fun temCores(b: Bitmap): Boolean {
        val vistas = HashSet<Int>()
        var y = 0
        while (y < b.height) {
            var x = 0
            while (x < b.width) {
                vistas += b.getPixel(x, y)
                if (vistas.size > 8) return true
                x += b.width / 16
            }
            y += b.height / 16
        }
        return false
    }

    @Test
    fun listaDeExemplo() {
        abrir("exemplo").use { cena ->
            capturar("dispositivos")
            vivo(cena)
        }
    }

    @Test
    fun modoDemonstracaoComToque() {
        device.setOrientationLeft()
        try {
            abrir("demo").use { cena ->
                capturar("sessao-demo-antes")
                // Toque no meio da tela: vira clique no host de demonstração.
                device.click(device.displayWidth / 2, device.displayHeight / 2)
                Thread.sleep(500)
                capturar("sessao-demo")
                vivo(cena)
            }
        } finally {
            device.setOrientationNatural()
        }
    }
}
