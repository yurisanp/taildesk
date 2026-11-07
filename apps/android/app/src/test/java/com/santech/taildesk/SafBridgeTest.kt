package com.santech.taildesk

import org.junit.Assert.assertEquals
import org.junit.Test

class SafBridgeTest {
    @Test
    fun nomeSemBarrasNemVazio() {
        assertEquals("foto.jpg", SafBridge.nomeSeguro("foto.jpg"))
        assertEquals("a_b.txt", SafBridge.nomeSeguro("a/b.txt"))
        assertEquals("a_b.txt", SafBridge.nomeSeguro("a\\b.txt"))
        assertEquals("arquivo", SafBridge.nomeSeguro(".."))
        assertEquals("arquivo", SafBridge.nomeSeguro(null))
        assertEquals("x y", SafBridge.nomeSeguro("x\ty"))
    }

    @Test
    fun linhaParaORust() {
        assertEquals("/c/1/f.txt\tf.txt\t12", SafBridge.linha("/c/1/f.txt", "f.txt", 12))
    }
}
