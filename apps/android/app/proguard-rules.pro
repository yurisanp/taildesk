# A biblioteca Rust chama estes métodos por nome (JNI): o R8 não pode renomear nem tirar.
-keep class com.santech.taildesk.MainActivity { public *; }
-keep class com.santech.taildesk.Ponte { *; }
# Helper Java do backend Android do Slint (carregado de dex embutido na .so).
-keep class dev.slint.** { *; }
