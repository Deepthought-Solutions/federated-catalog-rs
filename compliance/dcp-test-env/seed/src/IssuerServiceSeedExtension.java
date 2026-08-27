import com.nimbusds.jose.jwk.ECKey;
import org.eclipse.edc.identityhub.spi.participantcontext.IdentityHubParticipantContextService;
import org.eclipse.edc.identityhub.spi.participantcontext.model.IdentityHubParticipantContext;
import org.eclipse.edc.identityhub.spi.participantcontext.model.KeyDescriptor;
import org.eclipse.edc.identityhub.spi.participantcontext.model.KeyPairUsage;
import org.eclipse.edc.identityhub.spi.participantcontext.model.ParticipantManifest;
import org.eclipse.edc.runtime.metamodel.annotation.Extension;
import org.eclipse.edc.runtime.metamodel.annotation.Inject;
import org.eclipse.edc.spi.security.Vault;
import org.eclipse.edc.spi.system.ServiceExtension;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.text.ParseException;
import java.util.Set;

/**
 * Bootstrap extension for the "dcp-test-env" (see
 * federated-catalog-rs/compliance/dcp-test-env/README.md): creates the
 * "issuer" participant context on the Issuer Service launcher, using the
 * *same* keypair IdentityHubSeedExtension uses to sign the credential it
 * seeds directly into the holder's credential store.
 *
 * This is what makes the issuer genuinely real rather than just a string:
 * the seeded credential's "iss"/"kid" (did:web:<issuer-did-host>:issuer#issuer-key)
 * resolves to an actually-running Issuer Service instance publishing a
 * real DID document with the matching public key - a relying party
 * (Rust's future DCP verifier, or this environment's own validation
 * script) can genuinely resolve and check it, not just trust a
 * hardcoded assumption.
 *
 * What this does NOT do: drive the live, asynchronous DCP issuance
 * protocol (attestation/credential definitions, holder registration,
 * the credential-request handshake) between this Issuer Service and the
 * IdentityHub instance - see IdentityHubSeedExtension's doc comment for
 * why that choreography is skipped for this environment.
 */
@Extension(IssuerServiceSeedExtension.NAME)
public class IssuerServiceSeedExtension implements ServiceExtension {
    public static final String NAME = "dcp-test-env seed (IssuerService)";

    private static final String ISSUER_PARTICIPANT_ID = "issuer";
    private static final String ISSUER_KEY_ID = "issuer-key";

    @Inject
    private IdentityHubParticipantContextService participantContextService;

    @Inject
    private Vault vault;

    @Override
    public String name() {
        return NAME;
    }

    @Override
    public void prepare() {
        var didHost = require("SEED_ISSUER_DID_HOST");
        var keysDir = Path.of(require("SEED_KEYS_DIR"));
        var issuerDid = "did:web:" + didHost.replace(":", "%3A") + ":" + ISSUER_PARTICIPANT_ID;

        var existing = participantContextService.getParticipantContext(ISSUER_PARTICIPANT_ID);
        if (existing.succeeded() && existing.getContent() != null) {
            System.out.println(NAME + ": participant '" + ISSUER_PARTICIPANT_ID + "' already exists, skipping creation");
            return;
        }

        ECKey issuerKey;
        try {
            var json = Files.readString(keysDir.resolve(ISSUER_KEY_ID + "-private.jwk.json"));
            issuerKey = ECKey.parse(json);
        } catch (IOException | ParseException e) {
            throw new RuntimeException("Failed to read seed key " + ISSUER_KEY_ID, e);
        }

        var privateKeyAlias = ISSUER_KEY_ID + "-alias";
        var fullKeyId = issuerDid + "#" + ISSUER_KEY_ID;

        var vaultResult = vault.storeSecret(ISSUER_PARTICIPANT_ID, privateKeyAlias, issuerKey.toJSONString());
        if (vaultResult.failed()) {
            throw new RuntimeException("Failed to store issuer private key in vault: " + vaultResult.getFailureDetail());
        }

        var manifest = ParticipantManifest.Builder.newInstance()
                .participantContextId(ISSUER_PARTICIPANT_ID)
                .did(issuerDid)
                .active(true)
                .key(KeyDescriptor.Builder.newInstance()
                        .keyId(fullKeyId)
                        .privateKeyAlias(privateKeyAlias)
                        .publicKeyJwk(issuerKey.toPublicJWK().toJSONObject())
                        .usage(Set.of(KeyPairUsage.CREDENTIAL_SIGNING, KeyPairUsage.TOKEN_SIGNING))
                        .build())
                .build();

        participantContextService.createParticipantContext(manifest)
                .orElseThrow(f -> new RuntimeException("Failed to create issuer participant context: " + f.getFailureDetail()));

        // Same activation quirk as IdentityHubSeedExtension - see that
        // class's doc comment for why this explicit step is needed.
        var activation = participantContextService.updateParticipant(ISSUER_PARTICIPANT_ID, IdentityHubParticipantContext::activate);
        if (activation.failed()) {
            throw new RuntimeException("Failed to activate issuer participant context: " + activation.getFailureDetail());
        }

        System.out.println(NAME + ": created + activated participant '" + ISSUER_PARTICIPANT_ID + "' did=" + issuerDid);
    }

    private static String require(String envVar) {
        var value = System.getenv(envVar);
        if (value == null || value.isBlank()) {
            throw new RuntimeException("Missing required env var " + envVar + " for dcp-test-env seeding");
        }
        return value;
    }
}
