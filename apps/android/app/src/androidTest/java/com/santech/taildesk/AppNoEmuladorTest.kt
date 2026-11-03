package com.santech.taildesk

import android.content.Intent
import android.graphics.Bitmap
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
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
    private val pacote = instr.targetContext.packageName

    private fun abrir(modo: String) {
        val intent = Intent(instr.targetContext, MainActivity::class.java)
            .putExtra(MainActivity.EXTRA_MODO, modo)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        instr.targetContext.startActivity(intent)
        assertTrue("o app não abriu", device.wait(Until.hasObject(By.pkg(pacote).depth(0)), 20_000))
        // O Slint desenha no primeiro quadro depois de subir a biblioteca.
        Thread.sleep(4_000)
    }

    private fun capturar(nome: String) {
        val pasta = File(instr.targetContext.getExternalFilesDir(null), "capturas").apply { mkdirs() }
        val arquivo = File(pasta, "$nome.png")
        assertTrue("captura $nome", device.takeScreenshot(arquivo))
        val bmp = android.graphics.BitmapFactory.decodeFile(arquivo.path)
        assertNotNull(bmp)
        assertTrue("tela vazia em $nome", temCores(bmp))
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
        abrir("exemplo")
        capturar("dispositivos")
        assertTrue("o app caiu", device.hasObject(By.pkg(pacote)))
    }

    @Test
    fun modoDemonstracaoComToque() {
        device.setOrientationLeft()
        try {
            abrir("demo")
            // Toque no meio da tela: vira clique no host de demonstração.
            device.click(device.displayWidth / 2, device.displayHeight / 2)
            Thread.sleep(500)
            capturar("sessao-demo")
            assertTrue("o app caiu", device.hasObject(By.pkg(pacote)))
        } finally {
            device.setOrientationNatural()
        }
    }
}
