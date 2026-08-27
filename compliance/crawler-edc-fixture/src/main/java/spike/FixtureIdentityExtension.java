/*
 * Binds this fixture's trivial, always-succeeds IdentityService (see
 * FixtureNoopIdentityService's doc comment) - split out from
 * CatalogFixtureExtension into its own extension class deliberately:
 * ControlPlaneServicesExtension (core/control-plane, part of
 * controlplane-base-bom) has a hard @Inject on IdentityService, so
 * whatever extension provides it must have NO dependency on anything
 * ControlPlaneServicesExtension itself provides (AssetIndex,
 * PolicyDefinitionService, ContractDefinitionService, ...) - otherwise
 * BaseRuntime's boot-time topological sort finds a genuine cycle
 * (confirmed empirically: the first version of this fixture combined
 * identity-binding and seeding in one class and failed to boot with
 * org.eclipse.edc.boot.util.CyclicDependencyException, see
 * ../crawler-edc-integration-test.md).
 */
package spike;

import org.eclipse.edc.protocol.spi.DefaultParticipantIdExtractionFunction;
import org.eclipse.edc.runtime.metamodel.annotation.Extension;
import org.eclipse.edc.runtime.metamodel.annotation.Provider;
import org.eclipse.edc.runtime.metamodel.annotation.Provides;
import org.eclipse.edc.spi.iam.AudienceResolver;
import org.eclipse.edc.spi.iam.IdentityService;
import org.eclipse.edc.spi.result.Result;
import org.eclipse.edc.spi.system.ServiceExtension;
import org.eclipse.edc.spi.system.ServiceExtensionContext;

@Provides(IdentityService.class)
@Extension(FixtureIdentityExtension.NAME)
public class FixtureIdentityExtension implements ServiceExtension {

    public static final String NAME = "federated-catalog-rs crawler-edc-fixture: no-op identity";

    @Override
    public String name() {
        return NAME;
    }

    @Override
    public void initialize(ServiceExtensionContext context) {
        // Real EDC's DSP request handler 401s on a *missing* Authorization
        // header regardless of which IdentityService is bound
        // (DspRequestHandlerImpl short-circuits before ever calling one),
        // but places no format requirement on a present header's content
        // once a service is bound - see
        // ../crawler-edc-integration-test.md for the discovery trail.
        // Real DCP identity is explicitly out of scope for this step (see
        // ../benchmark-dcp-2026-08-27.md); this mirrors
        // eclipse-edc-connector's own
        // system-tests/tck/tck-extension/.../NoopIdentityService.
        context.registerService(IdentityService.class, new FixtureNoopIdentityService());
        context.getMonitor().info(NAME + ": registered FixtureNoopIdentityService (no real auth - out of scope, see module doc comment)");
    }

    @Provider(isDefault = true)
    public DefaultParticipantIdExtractionFunction participantIdExtractionFunction() {
        return claimToken -> claimToken.getStringClaim(FixtureNoopIdentityService.CLAIM_CLIENT_ID);
    }

    // Required by DspHttpCoreExtension (@Inject private AudienceResolver
    // audienceResolver) - no real DCP/OAuth2 audience resolution needed
    // for this fixture, so this just echoes the counterparty address
    // (matches iam-mock's own default AudienceResolver in
    // IamMockExtension.audienceResolver()).
    @Provider
    public AudienceResolver audienceResolver() {
        return message -> Result.success(message.getCounterPartyAddress());
    }
}
