package co.pagis.mobile;

import static org.junit.Assert.*;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import androidx.browser.customtabs.CustomTabsIntent;

@RunWith(RobolectricTestRunner.class)
public class SignInVerifierTest {
    @Test public void theCustomTabRequestsEphemeralBrowsing() {
        assertTrue(PagisShellPlugin.signInTab().intent.getBooleanExtra(CustomTabsIntent.EXTRA_ENABLE_EPHEMERAL_BROWSING, false));
    }

    @Test public void theVerifierGoesOnlyToItsOriginAndSecretAndOnlyOnce() {
        SignInVerifier.keep("https://pagis.example", "secret", "verifier");
        assertNull(SignInVerifier.take("https://other.example", "secret"));
        assertNull(SignInVerifier.take("https://pagis.example", "wrong"));
        assertEquals("verifier", SignInVerifier.take("https://pagis.example", "secret"));
        assertNull(SignInVerifier.take("https://pagis.example", "secret"));
    }
}
