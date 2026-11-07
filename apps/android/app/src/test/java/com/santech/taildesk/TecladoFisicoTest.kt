package com.santech.taildesk

import android.view.KeyCharacterMap
import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Teclado ABNT2 de um tablet (layout do aparelho já aplicado pelo Android). */
class TecladoFisicoTest {

    /** `getDeadChar` de mentira: só ´ + vogal. */
    private val teclado = TecladoFisico { acento, c ->
        if (acento == 0x0301) mapOf('a'.code to 'á'.code, 'e'.code to 'é'.code)[c] ?: 0 else 0
    }

    private val agudo = KeyCharacterMap.COMBINING_ACCENT or 0x0301

    private fun letra(c: Char, meta: Int = 0, semMeta: Char = c) =
        teclado.traduzir(KeyEvent.KEYCODE_A, meta, c.code, semMeta.code)

    @Test
    fun texto_com_layout_e_tecla_morta() {
        assertEquals("texto\tç\t", letra('ç'))
        assertEquals("texto\tA\t", letra('A', KeyEvent.META_SHIFT_ON, 'a'))
        // ´ espera a vogal; ´ + espaço sai sozinho; ´ + consoante sai junto.
        assertEquals("", teclado.traduzir(KeyEvent.KEYCODE_LEFT_BRACKET, 0, agudo, agudo))
        assertEquals("texto\tá\t", letra('a'))
        teclado.traduzir(KeyEvent.KEYCODE_LEFT_BRACKET, 0, agudo, agudo)
        assertEquals("texto\t´\t", letra(' '))
        teclado.traduzir(KeyEvent.KEYCODE_LEFT_BRACKET, 0, agudo, agudo)
        assertEquals("texto\t´x\t", letra('x'))
        // Acento pendente antes de uma tecla sem texto: sai antes dela.
        teclado.traduzir(KeyEvent.KEYCODE_LEFT_BRACKET, 0, agudo, agudo)
        assertEquals(
            "texto\t´\t\ntecla\tenter\t",
            teclado.traduzir(KeyEvent.KEYCODE_ENTER, 0, '\n'.code, '\n'.code),
        )
    }

    @Test
    fun atalhos_e_teclas_sem_texto() {
        val ctrl = KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
        // Com Ctrl o Android dá 0 em getUnicodeChar(meta); vale a letra sem modificador.
        assertEquals("texto\tc\tc", teclado.traduzir(KeyEvent.KEYCODE_C, ctrl, 0, 'c'.code))
        val ctrlShift = ctrl or KeyEvent.META_SHIFT_ON
        assertEquals("texto\tt\tcs", teclado.traduzir(KeyEvent.KEYCODE_T, ctrlShift, 0, 't'.code))
        val altEsq = KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON
        assertEquals("tecla\tf4\ta", teclado.traduzir(KeyEvent.KEYCODE_F4, altEsq, 0, 0))
        assertEquals(
            "tecla\tesquerda\ts",
            teclado.traduzir(KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.META_SHIFT_ON, 0, 0),
        )
        assertEquals("tecla\tbackspace\t", teclado.traduzir(KeyEvent.KEYCODE_DEL, 0, 0, 0))
        // AltGr+Q no ABNT2 é "/": texto, sem Alt.
        val altGr = KeyEvent.META_ALT_ON or KeyEvent.META_ALT_RIGHT_ON
        assertEquals("texto\t/\t", teclado.traduzir(KeyEvent.KEYCODE_Q, altGr, '/'.code, 'q'.code))
    }

    @Test
    fun modificador_sozinho_e_teclas_do_sistema() {
        assertEquals("", teclado.traduzir(KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.META_SHIFT_ON, 0, 0))
        assertEquals("", teclado.traduzir(KeyEvent.KEYCODE_CTRL_LEFT, 0, 0, 0))
        assertNull(teclado.traduzir(KeyEvent.KEYCODE_BACK, 0, 0, 0))
        assertNull(teclado.traduzir(KeyEvent.KEYCODE_VOLUME_UP, 0, 0, 0))
    }
}
