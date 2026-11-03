import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

// Assinatura: variáveis de ambiente no CI (secrets TAILDESK_ANDROID_*) ou keystore.properties
// local, fora do git (android-detalhado.md §9.2). Sem nenhum dos dois o release sai sem
// assinatura (serve para o CI pegar erro de R8 e de lint cedo).
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

fun segredo(env: String, prop: String): String? =
    System.getenv(env)?.takeIf { it.isNotBlank() } ?: keystoreProps.getProperty(prop)

val keystorePath = segredo("TAILDESK_ANDROID_KEYSTORE", "storeFile")
val repoRoot: File = rootProject.projectDir.parentFile.parentFile
val abis: List<String> = (findProperty("taildesk.abis") as String? ?: "arm64-v8a,x86_64")
    .split(",").map { it.trim() }.filter { it.isNotEmpty() }

android {
    namespace = "com.santech.taildesk"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "com.santech.taildesk"
        minSdk = 29
        targetSdk = 36
        // O versionCode vem do número da execução do workflow "Versão para a Play".
        versionCode = System.getenv("TAILDESK_ANDROID_VERSION_CODE")?.toIntOrNull() ?: 1
        versionName = "0.3.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            abiFilters += abis
            // Símbolos da biblioteca Rust para a Play (relatórios de falha legíveis).
            debugSymbolLevel = "SYMBOL_TABLE"
        }
    }

    signingConfigs {
        if (keystorePath != null) {
            create("envio") {
                storeFile = file(keystorePath)
                storePassword = segredo("TAILDESK_ANDROID_KEYSTORE_PASSWORD", "storePassword")
                keyAlias = segredo("TAILDESK_ANDROID_KEY_ALIAS", "keyAlias")
                keyPassword = segredo("TAILDESK_ANDROID_KEY_PASSWORD", "keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("envio")
        }
        debug {
            applicationIdSuffix = ".debug"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    androidResources {
        // pt-BR e en (regra do projeto): o Android escolhe pelo idioma do sistema.
        generateLocaleConfig = true
    }

    bundle {
        // O idioma pode mudar dentro do app sem baixar outro pedaço do bundle.
        language { enableSplit = false }
    }

    lint {
        abortOnError = true
        warningsAsErrors = false
        checkReleaseBuilds = true
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
    }

    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs"))

    packaging {
        // Os .so vão sem strip: o AGP tira os símbolos e guarda para a Play.
        jniLibs.keepDebugSymbols += "**/libtaildesk_mobile.so"
    }
}

kotlin {
    jvmToolchain(17)
}

// Biblioteca Rust (crates/taildesk-mobile) pelo cargo-ndk, antes de juntar as libs nativas.
fun cargoNdk(nome: String, release: Boolean) = tasks.register<Exec>(nome) {
    group = "rust"
    description = "Compila libtaildesk_mobile.so (${if (release) "release" else "debug"}) para $abis"
    workingDir = repoRoot
    val saida = layout.buildDirectory.dir("rustJniLibs").get().asFile
    val args = mutableListOf("cargo", "ndk", "--platform", "29", "-o", saida.absolutePath)
    abis.forEach { args += listOf("-t", it) }
    args += listOf("build", "-p", "taildesk-mobile")
    if (release) args += "--release"
    // Debug otimizado o bastante para o vídeo ser usável no emulador (android-detalhado §4.2).
    if (!release) environment("CARGO_PROFILE_DEV_OPT_LEVEL", "1")
    commandLine(args)
    inputs.dir(File(repoRoot, "crates"))
    inputs.file(File(repoRoot, "Cargo.lock"))
    outputs.dir(saida)
}

val cargoDebug = cargoNdk("cargoBuildDebug", release = false)
val cargoRelease = cargoNdk("cargoBuildRelease", release = true)

tasks.configureEach {
    when {
        name.startsWith("merge") && name.endsWith("JniLibFolders") ->
            dependsOn(if (name.contains("Release")) cargoRelease else cargoDebug)
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity)
    implementation(libs.camerax.camera2)
    implementation(libs.camerax.lifecycle)
    implementation(libs.camerax.view)

    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.androidx.test.core)

    androidTestImplementation(libs.androidx.junit)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.rules)
    androidTestImplementation(libs.uiautomator)
}
