// Throwaway fixture: a real Eclipse EDC 0.18.0 control-plane runtime built
// entirely from published Maven Central artifacts (no vendored source
// touched, no edc-build-plugin), used to prove crates/crawler's HTTP
// client / DSP-response parser against a real connector, not just this
// workspace's own http-api. See ../crawler-edc-integration-test.md.
//
// One JVM launcher (spike.CatalogFixtureExtension, discovered via the
// plain java.util.ServiceLoader file under
// src/main/resources/META-INF/services/ - BaseRuntime's own extension
// discovery mechanism, confirmed by reading
// core/common/boot/src/main/java/org/eclipse/edc/boot/system/ServiceLocatorImpl.java
// in the `dataspace` study repo's vendored eclipse-edc-connector) is
// reused to start three independent, differently-seeded instances - see
// run-instance.sh.
plugins {
    java
}

repositories {
    mavenCentral()
}

dependencies {
    // A real, published aggregator - not just a <dependencyManagement>
    // BOM: org.eclipse.edc:controlplane-base-bom:0.18.0's own POM lists
    // compile-scope dependencies on core-spi/boot/connector-core/dsp/...
    // directly, confirmed by fetching it from repo1.maven.org before
    // depending on it (same as the `dataspace` study repo's
    // 2026-08-27-edc-catalog-metadata-exposure-policy.md spike did).
    implementation("org.eclipse.edc:controlplane-base-bom:0.18.0")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

// Writes the full runtime classpath (this project's own compiled
// classes/resources + every resolved dependency jar) to build/classpath.txt,
// colon-joined, so a plain `java -cp "$(cat build/classpath.txt)" \
// org.eclipse.edc.boot.system.runtime.BaseRuntime` can launch the runtime
// directly - no Gradle daemon needed for the three long-running instances
// themselves, only for this one-time classpath resolution.
tasks.register("printClasspath") {
    dependsOn(tasks.named("classes"))
    doLast {
        val cp = sourceSets["main"].runtimeClasspath.files.joinToString(":") { it.absolutePath }
        val out = layout.buildDirectory.file("classpath.txt").get().asFile
        out.parentFile.mkdirs()
        out.writeText(cp)
        println("wrote ${out.absolutePath} (${sourceSets["main"].runtimeClasspath.files.size} entries)")
    }
}
