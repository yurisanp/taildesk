package com.santech.taildesk

import android.content.Intent
import android.net.Uri
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class ShareReceiverTest {
    private val a = Uri.parse("content://fotos/1")
    private val b = Uri.parse("content://fotos/2")

    @Test
    fun umArquivo() {
        val i = Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, a)
        assertEquals(listOf(a), ShareReceiverActivity.urisCompartilhadas(i))
    }

    @Test
    fun variosArquivos() {
        val i = Intent(Intent.ACTION_SEND_MULTIPLE)
            .putParcelableArrayListExtra(Intent.EXTRA_STREAM, arrayListOf(a, b))
        assertEquals(listOf(a, b), ShareReceiverActivity.urisCompartilhadas(i))
    }

    @Test
    fun textoSemArquivoNaoConta() {
        val i = Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_TEXT, "oi")
        assertTrue(ShareReceiverActivity.urisCompartilhadas(i).isEmpty())
        assertTrue(ShareReceiverActivity.urisCompartilhadas(null).isEmpty())
    }
}
