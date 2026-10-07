package app.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/**
 * The copy of the Session follows the Session cookie of the server in the
 * cookie store of the web view, and goes back into the store at launch
 * (ADR-0032).
 */
public class SessionCookiesTest {

    @Rule
    public final TemporaryFolder folder = new TemporaryFolder();

    private static final long NOW = 1_800_000_000_000L;
    private static final long THIRTY_DAYS = TimeUnit.DAYS.toMillis(30);

    private final ServerOrigin server = ServerOrigin.parse("https://a.example", false);
    private SessionCopy copy;
    private FakeCookieJar jar;

    @Before
    public void makeCopy() throws Exception {
        copy = new SessionCopy(folder.newFile("session"), SessionCopyTest.testAead());
        jar = new FakeCookieJar();
    }

    // --- the follower ---

    /** The cookie store gives no expiry, so the copy lasts the life that
     *  the daemon gives a Session from its last use. */
    @Test
    public void aReadOfTheStoreWritesTheCopy() {
        jar.cookies.put("https://a.example", "theme=dark; pagis_session=s1");

        SessionCookies.follow(server, jar, copy, NOW);

        assertEquals(new Session("s1", NOW + THIRTY_DAYS), copy.read(server));
    }

    @Test
    public void aRemovedCookieDeletesTheCopy() {
        copy.write(server, new Session("s1", NOW + THIRTY_DAYS));
        jar.cookies.put("https://a.example", "theme=dark");

        SessionCookies.follow(server, jar, copy, NOW);

        assertNull(copy.read(server));
    }

    @Test
    public void aCookieOfAnotherHostIsIgnored() {
        jar.cookies.put("https://b.example", "pagis_session=b");
        jar.cookies.put("https://x.a.example", "pagis_session=x");

        SessionCookies.follow(server, jar, copy, NOW);
        assertNull(copy.read(server));

        jar.cookies.put("https://a.example", "pagis_session=a");
        SessionCookies.follow(server, jar, copy, NOW);
        assertEquals("a", copy.read(server).value);
    }

    @Test
    public void aCookieWhoseNameOnlyStartsLikeTheSessionCookieIsNotIt() {
        jar.cookies.put("https://a.example", "pagis_session_old=x; xpagis_session=y");

        SessionCookies.follow(server, jar, copy, NOW);

        assertNull(copy.read(server));
    }

    // --- the launch ---

    @Test
    public void aLaunchWithALiveCopyAndAnEmptyStoreWritesTheCookie() {
        copy.write(server, new Session("s1", 1_800_086_400_000L));

        SessionCookies.restore(server, jar, copy, NOW);

        assertEquals(1, jar.written.size());
        assertEquals("https://a.example", jar.written.get(0)[0]);
        assertEquals(
            "pagis_session=s1; Path=/; Expires=Sat, 16 Jan 2027 08:00:00 GMT; HttpOnly; SameSite=Strict; Secure",
            jar.written.get(0)[1]
        );
    }

    /** A debug build reaches a daemon over http:// on loopback, where the
     *  cookie has no Secure. */
    @Test
    public void theCookieOfAnHttpOriginIsNotSecure() {
        ServerOrigin loopback = ServerOrigin.parse("http://127.0.0.1:4400", true);
        copy.write(loopback, new Session("s1", 1_800_086_400_000L));

        SessionCookies.restore(loopback, jar, copy, NOW);

        assertEquals("http://127.0.0.1:4400", jar.written.get(0)[0]);
        assertTrue(jar.written.get(0)[1].endsWith("; HttpOnly; SameSite=Strict"));
    }

    @Test
    public void aLaunchKeepsACookieThatTheStoreHolds() {
        copy.write(server, new Session("copy", NOW + THIRTY_DAYS));
        jar.cookies.put("https://a.example", "pagis_session=store");

        SessionCookies.restore(server, jar, copy, NOW);

        assertTrue(jar.written.isEmpty());
    }

    @Test
    public void aLaunchWritesNoExpiredCopy() {
        copy.write(server, new Session("s1", NOW));

        SessionCookies.restore(server, jar, copy, NOW);

        assertTrue(jar.written.isEmpty());
    }

    @Test
    public void aLaunchWithNoCopyWritesNothing() {
        SessionCookies.restore(server, jar, copy, NOW);

        assertTrue(jar.written.isEmpty());
    }

    /** A cookie store in memory, by URL, as {@code CookieManager} answers. */
    private static final class FakeCookieJar implements CookieJar {
        final Map<String, String> cookies = new HashMap<>();
        final List<String[]> written = new ArrayList<>();

        @Override
        public String getCookie(String url) {
            return cookies.get(url);
        }

        @Override
        public void setCookie(String url, String cookie) {
            written.add(new String[] { url, cookie });
        }
    }
}
