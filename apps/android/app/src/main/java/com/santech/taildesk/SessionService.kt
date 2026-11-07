package com.santech.taildesk

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

/**
 * Som da sessão em segundo plano (MF-72): enquanto o app está fora da tela com o som ligado, um
 * serviço em primeiro plano do tipo `mediaPlayback` mantém o processo vivo e mostra
 * "Ouvindo <computador> · Encerrar". O Rust liga e desliga por [MainActivity.ouvindo].
 */
class SessionService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACAO_ENCERRAR) {
            if (Ponte.carregada) Ponte.nativeEncerrar()
            parar()
            return START_NOT_STICKY
        }
        val nome = intent?.getStringExtra(EXTRA_NOME).orEmpty()
        criarCanal(this)
        val abrir = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val encerrar = PendingIntent.getService(
            this,
            1,
            Intent(this, SessionService::class.java).setAction(ACAO_ENCERRAR),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val n = NotificationCompat.Builder(this, CANAL)
            .setSmallIcon(R.drawable.ic_som)
            .setContentTitle(getString(R.string.ouvindo, nome))
            .setContentIntent(abrir)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
            .addAction(0, getString(R.string.encerrar), encerrar)
            .build()
        ServiceCompat.startForeground(
            this,
            ID,
            n,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK,
        )
        return START_NOT_STICKY
    }

    private fun parar() {
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    companion object {
        const val CANAL = "som"
        const val ID = 72
        const val ACAO_ENCERRAR = "com.santech.taildesk.ENCERRAR_SOM"
        const val EXTRA_NOME = "nome"

        fun criarCanal(context: Context) {
            val nm = context.getSystemService(NotificationManager::class.java)
            val canal = NotificationChannel(
                CANAL,
                context.getString(R.string.canal_som),
                NotificationManager.IMPORTANCE_LOW,
            )
            canal.description = context.getString(R.string.canal_som_desc)
            nm.createNotificationChannel(canal)
        }

        /** Liga (com o nome do computador) ou desliga (`null`) a notificação. */
        fun ouvindo(context: Context, nome: String?) {
            val i = Intent(context, SessionService::class.java)
            if (nome == null) {
                context.stopService(i)
            } else {
                context.startForegroundService(i.putExtra(EXTRA_NOME, nome))
            }
        }
    }
}
