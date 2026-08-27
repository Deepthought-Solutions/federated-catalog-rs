/*
 * Throwaway fixture extension for federated-catalog-rs's compliance
 * suite - proves crates/crawler against a real Eclipse EDC 0.18.0
 * control-plane runtime. NOT a general-purpose EDC extension.
 *
 * Seeding only - see FixtureIdentityExtension (a separate class,
 * deliberately: see its doc comment for why combining the two caused a
 * boot-time CyclicDependencyException) for the no-op IdentityService
 * this fixture registers instead of real DCP (out of scope here, see
 * ../benchmark-dcp-2026-08-27.md and ../crawler-edc-integration-test.md).
 *
 * Env-var-parameterized so the same built classpath launches three
 * independent, differently-seeded instances (see run-instance.sh):
 *
 *   FIXTURE_ASSET_IDS   - required. Comma-separated dataset ids to seed,
 *                         e.g. "EDC-A-01,EDC-A-02". Each gets a trivial
 *                         HttpData DataAddress (never actually dialed -
 *                         only the DSP catalog listing is exercised) and
 *                         is offered under one shared, unconstrained
 *                         "use" policy + one contract definition with an
 *                         empty assetsSelector (selects every asset in
 *                         this participant context - the same pattern
 *                         eclipse-edc-connector's own
 *                         system-tests/tck/tck-extension DataSeed.java
 *                         uses for its 17-asset seed).
 *
 * Participant context id is read from EDC_PARTICIPANT_CONTEXT_ID (or,
 * if unset, EDC_PARTICIPANT_ID) directly via System.getenv() rather than
 * ServiceExtensionContext settings - this fixture always sets both env
 * vars identically per instance (see run-instance.sh), sidestepping the
 * default-fallback mismatch documented in
 * compliance/benchmark-2026-08-27.md's "Problems encountered" #2.
 */
package spike;

import org.eclipse.edc.connector.controlplane.asset.spi.domain.Asset;
import org.eclipse.edc.connector.controlplane.asset.spi.index.AssetIndex;
import org.eclipse.edc.connector.controlplane.contract.spi.types.offer.ContractDefinition;
import org.eclipse.edc.connector.controlplane.policy.spi.PolicyDefinition;
import org.eclipse.edc.connector.controlplane.services.spi.contractdefinition.ContractDefinitionService;
import org.eclipse.edc.connector.controlplane.services.spi.policydefinition.PolicyDefinitionService;
import org.eclipse.edc.policy.model.Action;
import org.eclipse.edc.policy.model.Permission;
import org.eclipse.edc.policy.model.Policy;
import org.eclipse.edc.runtime.metamodel.annotation.Extension;
import org.eclipse.edc.runtime.metamodel.annotation.Inject;
import org.eclipse.edc.spi.system.ServiceExtension;
import org.eclipse.edc.spi.system.ServiceExtensionContext;
import org.eclipse.edc.spi.types.domain.DataAddress;

import java.util.ArrayList;
import java.util.List;

@Extension(CatalogFixtureExtension.NAME)
public class CatalogFixtureExtension implements ServiceExtension {

    public static final String NAME = "federated-catalog-rs crawler-edc-fixture: catalog seed";

    private static final String OPEN_POLICY_ID = "fixture-open-use-policy";
    private static final String OPEN_CONTRACT_DEFINITION_ID = "fixture-open-contract-definition";

    @Inject
    private AssetIndex assetIndex;

    @Inject
    private PolicyDefinitionService policyDefinitionService;

    @Inject
    private ContractDefinitionService contractDefinitionService;

    private ServiceExtensionContext context;

    @Override
    public String name() {
        return NAME;
    }

    @Override
    public void initialize(ServiceExtensionContext context) {
        this.context = context;
    }

    @Override
    public void prepare() {
        var participantContextId = participantContextId();
        var assetIds = fixtureAssetIds();
        var monitor = context.getMonitor();

        for (var assetId : assetIds) {
            var asset = Asset.Builder.newInstance()
                    .id(assetId)
                    .property("fixtureAssetName", assetId)
                    .dataAddress(DataAddress.Builder.newInstance()
                            .type("HttpData")
                            .property("baseUrl", "https://example.invalid/fixture/" + assetId)
                            .build())
                    .participantContextId(participantContextId)
                    .build();
            var result = assetIndex.create(asset);
            if (result.failed()) {
                throw new IllegalStateException("failed to seed fixture asset '" + assetId + "': " + result.getFailureDetail());
            }
        }

        var permission = Permission.Builder.newInstance()
                .action(Action.Builder.newInstance().type("http://www.w3.org/ns/odrl/2/use").build())
                .build();
        var policyDefinition = PolicyDefinition.Builder.newInstance()
                .id(OPEN_POLICY_ID)
                .policy(Policy.Builder.newInstance().permission(permission).build())
                .participantContextId(participantContextId)
                .build();
        var policyResult = policyDefinitionService.create(policyDefinition);
        if (policyResult.failed()) {
            throw new IllegalStateException("failed to seed fixture policy: " + policyResult.getFailureDetail());
        }

        // No assetsSelector set - an empty selector matches every asset in
        // this participant context (confirmed by reading
        // eclipse-edc-connector's own DataSeed.createContractDefinitions,
        // which relies on the same empty-selector-selects-all behavior for
        // its 17-asset TCK seed).
        var contractDefinition = ContractDefinition.Builder.newInstance()
                .id(OPEN_CONTRACT_DEFINITION_ID)
                .accessPolicyId(OPEN_POLICY_ID)
                .contractPolicyId(OPEN_POLICY_ID)
                .participantContextId(participantContextId)
                .build();
        var contractDefinitionResult = contractDefinitionService.create(contractDefinition);
        if (contractDefinitionResult.failed()) {
            throw new IllegalStateException("failed to seed fixture contract definition: " + contractDefinitionResult.getFailureDetail());
        }

        monitor.info(NAME + ": seeded " + assetIds.size() + " asset(s) " + assetIds
                + " under participantContextId='" + participantContextId + "'");
    }

    private static String participantContextId() {
        var env = System.getenv();
        var explicit = env.get("EDC_PARTICIPANT_CONTEXT_ID");
        if (explicit != null && !explicit.isBlank()) {
            return explicit;
        }
        var fallback = env.get("EDC_PARTICIPANT_ID");
        if (fallback != null && !fallback.isBlank()) {
            return fallback;
        }
        throw new IllegalStateException("neither EDC_PARTICIPANT_CONTEXT_ID nor EDC_PARTICIPANT_ID is set - both this fixture's seeding and the runtime's own single-participant context resolution need one, and they must agree (see module doc comment)");
    }

    private static List<String> fixtureAssetIds() {
        var raw = System.getenv("FIXTURE_ASSET_IDS");
        if (raw == null || raw.isBlank()) {
            throw new IllegalStateException("FIXTURE_ASSET_IDS is not set - expected a comma-separated list of dataset ids to seed, e.g. \"EDC-A-01,EDC-A-02\"");
        }
        var ids = new ArrayList<String>();
        for (var part : raw.split(",")) {
            var trimmed = part.trim();
            if (!trimmed.isEmpty()) {
                ids.add(trimmed);
            }
        }
        if (ids.isEmpty()) {
            throw new IllegalStateException("FIXTURE_ASSET_IDS was set but contained no non-empty entries: '" + raw + "'");
        }
        return ids;
    }
}
