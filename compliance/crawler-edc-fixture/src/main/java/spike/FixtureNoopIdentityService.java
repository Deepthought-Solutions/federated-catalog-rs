/*
 * A trivial, always-succeeds IdentityService for this compliance
 * fixture only - modeled on eclipse-edc-connector's own
 * system-tests/tck/tck-extension/.../identity/NoopIdentityService.java
 * (read in the `dataspace` study repo's vendored copy for reference).
 * Real DCP identity is out of scope for this step - see
 * CatalogFixtureExtension's module doc comment and
 * compliance/benchmark-dcp-2026-08-27.md.
 *
 * Does not parse or validate the Authorization header's content at all
 * (unlike EDC's first-party iam-mock, which JSON-decodes it) - any
 * non-null token is accepted. This matters because real EDC's DSP
 * request handler (DspRequestHandlerImpl) 401s on a *missing*
 * Authorization header before an IdentityService ever runs, but places
 * no format requirement on a present one once a service is bound.
 */
package spike;

import org.eclipse.edc.spi.iam.ClaimToken;
import org.eclipse.edc.spi.iam.IdentityService;
import org.eclipse.edc.spi.iam.TokenParameters;
import org.eclipse.edc.spi.iam.TokenRepresentation;
import org.eclipse.edc.spi.iam.VerificationContext;
import org.eclipse.edc.spi.result.Result;

public class FixtureNoopIdentityService implements IdentityService {

    static final String CLAIM_CLIENT_ID = "client_id";
    private static final String FIXTURE_CALLER_ID = "FIXTURE_CRAWLER";

    @Override
    public Result<TokenRepresentation> obtainClientCredentials(String participantContextId, TokenParameters tokenParameters) {
        return Result.success(TokenRepresentation.Builder.newInstance().token("fixture-noop-token").expiresIn(Long.MAX_VALUE).build());
    }

    @Override
    public Result<ClaimToken> verifyJwtToken(String participantContextId, TokenRepresentation tokenRepresentation, VerificationContext verificationContext) {
        return Result.success(ClaimToken.Builder.newInstance().claim(CLAIM_CLIENT_ID, FIXTURE_CALLER_ID).build());
    }
}
