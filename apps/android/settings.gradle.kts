// App Android do TailDesk (docs/context/mobile/android-detalhado.md). O núcleo é Rust
// (crates/taildesk-mobile), compilado pelo cargo-ndk antes de empacotar.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "TailDesk"
include(":app")
