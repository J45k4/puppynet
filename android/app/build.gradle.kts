plugins {
	id("com.android.application")
	id("org.jetbrains.kotlin.android")
}

android {
	namespace = "fi.puppycorp.puppynet"
	compileSdk = 35

	defaultConfig {
		applicationId = "fi.puppycorp.puppynet"
		minSdk = 26
		targetSdk = 35
		versionCode = 1
		versionName = "0.1.0"
	}

	buildFeatures {
		compose = true
	}
}

dependencies {
	implementation(platform("androidx.compose:compose-bom:2024.12.01"))
	implementation("androidx.activity:activity-compose:1.10.0")
	implementation("androidx.compose.material3:material3")
	implementation("androidx.compose.ui:ui")
	implementation("androidx.compose.ui:ui-tooling-preview")
	implementation("com.google.android.gms:play-services-code-scanner:16.1.0")
	implementation("com.squareup.okhttp3:okhttp:4.12.0")
	debugImplementation("androidx.compose.ui:ui-tooling")
}

tasks.register<Exec>("buildRust") {
	workingDir = file("../native")
	commandLine(
		"cargo",
		"ndk",
		"-t",
		"arm64-v8a",
		"-o",
		file("src/main/jniLibs").absolutePath,
		"build",
		"--release",
	)
}

tasks.named("preBuild").configure {
	dependsOn("buildRust")
}
