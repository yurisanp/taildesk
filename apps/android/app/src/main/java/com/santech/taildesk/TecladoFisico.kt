package com.santech.taildesk

import android.view.InputDevice
import android.view.KeyCharacterMap
import android.view.KeyEvent

/**
 * Teclado físico (USB ou Bluetooth) durante a sessão (M3). O Slint recebe as teclas primeiro, mas
 * só com o mapa americano; o que ele não aceita chega aqui com o layout do aparelho, e vira uma
 * linha para `Ponte.nativeTeclaFisica` (formato em `crates/taildesk-mobile/src/fisico.rs`):
 * `tecla\t<nome>\t<mods>` ou `texto\t<texto>\t<mods>`, com `c` Ctrl, `a` Alt, `s` Shift e
 * `m` Meta.
 *
 * Modo Auto de `11-teclado.md`: tecla sem texto pelo nome; com Ctrl, Alt ou Meta, atalho pela letra
 * do layout; o resto como caractere, com as teclas mortas resolvidas aqui.
 *
 * [combinar] é o `KeyEvent.getDeadChar` (trocado nos testes).
 */
class TecladoFisico(private val combinar: (Int, Int) -> Int = KeyEvent::getDeadChar) {

    /** Acento de uma tecla morta esperando a próxima letra (0 = nenhum). */
    private var morta = 0

    fun limpar() {
        morta = 0
    }

    /**
     * Tecla apertada. `null` = não é nossa (o sistema cuida); vazio = engolir sem mandar nada
     * (modificador sozinho, tecla morta); senão a linha para a ponte.
     *
     * [comMeta] é `getUnicodeChar(metaState)` e [semMeta] é `getUnicodeChar(0)`.
     */
    fun traduzir(keyCode: Int, meta: Int, comMeta: Int, semMeta: Int): String? {
        if (keyCode in MODIFICADORES) return ""
        val ctrl = (meta and KeyEvent.META_CTRL_ON) != 0
        val shift = (meta and KeyEvent.META_SHIFT_ON) != 0
        val metaOn = (meta and KeyEvent.META_META_ON) != 0
        val altEsq = (meta and KeyEvent.META_ALT_LEFT_ON) != 0
        // AltGr sozinho (Alt direito) com caractere é texto (AltGr+Q = "/" no ABNT2).
        val altGr = (meta and KeyEvent.META_ALT_RIGHT_ON) != 0 && !altEsq && !ctrl && !metaOn
        val alt = altEsq || ((meta and KeyEvent.META_ALT_RIGHT_ON) != 0 && !altGr)
        val mods = (if (ctrl) "c" else "") + (if (alt || (altGr && comMeta == 0)) "a" else "") +
            (if (shift) "s" else "") + (if (metaOn) "m" else "")

        NOMES[keyCode]?.let { nome ->
            val pendente = soltarMorta()
            return pendente + "tecla\t$nome\t$mods"
        }
        if (ctrl || alt || metaOn) {
            val c = semMeta and KeyCharacterMap.COMBINING_ACCENT_MASK
            if (c == 0) return null
            morta = 0
            return "texto\t${texto(c)}\t$mods"
        }
        if (comMeta == 0) return null
        if ((comMeta and KeyCharacterMap.COMBINING_ACCENT) != 0) {
            val acento = comMeta and KeyCharacterMap.COMBINING_ACCENT_MASK
            // Duas mortas seguidas: a primeira sai sozinha.
            val antes = if (morta != 0) "texto\t${espacado(morta)}\t" else ""
            morta = acento
            return antes
        }
        if (morta == 0) return "texto\t${texto(comMeta)}\t"
        val acento = morta
        morta = 0
        val junto = if (comMeta == ' '.code) 0 else combinar(acento, comMeta)
        return when {
            comMeta == ' '.code -> "texto\t${espacado(acento)}\t"
            junto != 0 -> "texto\t${texto(junto)}\t"
            else -> "texto\t${espacado(acento)}${texto(comMeta)}\t"
        }
    }

    /** O acento pendente sai antes de uma tecla sem texto (linha + quebra), ou nada. */
    private fun soltarMorta(): String {
        if (morta == 0) return ""
        val s = espacado(morta)
        morta = 0
        // Duas linhas numa chamada só: a ponte separa por quebra de linha.
        return "texto\t$s\t\n"
    }

    companion object {
        private fun texto(c: Int) = String(Character.toChars(c))

        /** Acento combinante → o caractere visível (´ ` ^ ~ ¨). */
        private fun espacado(c: Int): String = texto(
            when (c) {
                0x0300 -> '`'.code
                0x0301 -> 0x00B4
                0x0302 -> '^'.code
                0x0303 -> '~'.code
                0x0308 -> 0x00A8
                else -> c
            }
        )

        private val MODIFICADORES = setOf(
            KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_SHIFT_RIGHT,
            KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_CTRL_RIGHT,
            KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_ALT_RIGHT,
            KeyEvent.KEYCODE_META_LEFT, KeyEvent.KEYCODE_META_RIGHT,
            KeyEvent.KEYCODE_CAPS_LOCK, KeyEvent.KEYCODE_NUM_LOCK,
            KeyEvent.KEYCODE_SCROLL_LOCK, KeyEvent.KEYCODE_FUNCTION,
        )

        /** Teclas sem texto, com o nome da faixa de teclas especiais. */
        private val NOMES: Map<Int, String> = buildMap {
            put(KeyEvent.KEYCODE_ESCAPE, "esc")
            put(KeyEvent.KEYCODE_TAB, "tab")
            put(KeyEvent.KEYCODE_ENTER, "enter")
            put(KeyEvent.KEYCODE_NUMPAD_ENTER, "enter")
            put(KeyEvent.KEYCODE_DEL, "backspace")
            put(KeyEvent.KEYCODE_FORWARD_DEL, "del")
            put(KeyEvent.KEYCODE_INSERT, "insert")
            put(KeyEvent.KEYCODE_DPAD_LEFT, "esquerda")
            put(KeyEvent.KEYCODE_DPAD_RIGHT, "direita")
            put(KeyEvent.KEYCODE_DPAD_UP, "cima")
            put(KeyEvent.KEYCODE_DPAD_DOWN, "baixo")
            put(KeyEvent.KEYCODE_MOVE_HOME, "home")
            put(KeyEvent.KEYCODE_MOVE_END, "end")
            put(KeyEvent.KEYCODE_PAGE_UP, "pgup")
            put(KeyEvent.KEYCODE_PAGE_DOWN, "pgdn")
            for (n in 1..12) put(KeyEvent.KEYCODE_F1 + n - 1, "f$n")
        }

        /** Veio de um teclado de verdade (não do teclado na tela). */
        fun eFisico(event: KeyEvent): Boolean {
            val d = event.device ?: return false
            return !d.isVirtual && event.isFromSource(InputDevice.SOURCE_KEYBOARD)
        }
    }
}
