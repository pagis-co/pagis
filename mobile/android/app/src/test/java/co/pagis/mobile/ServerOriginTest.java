package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The origin match of the Mobile App: the scheme, the host and the port
 * must all match.
 */
public class ServerOriginTest {

    private final ServerOrigin server = ServerOrigin.parse("https://a.example", false);

    @Test
    public void theServerOriginMatchesItself() {
        assertNotNull(server);
        assertTrue(server.matches("https://a.example"));
        assertTrue(server.matches("https://a.example/"));
        assertTrue(server.matches("HTTPS://A.Example:443"));
    }

    @Test
    public void anotherPortSchemeOrHostDoesNotMatch() {
        assertFalse(server.matches("https://a.example:444"));
        assertFalse(server.matches("http://a.example"));
        assertFalse(server.matches("https://b.a.example"));
        assertFalse(server.matches("not a url"));
    }

    /** Capacitor makes the allowed origin rule of its bridge from the
     *  server URL, and a rule with a path turns that check off. */
    @Test
    public void theServerUrlHasNoPathAndNoTrailingSlash() {
        assertEquals("https://a.example", ServerOrigin.parse("https://a.example/", false).serverUrl());
        assertEquals("https://a.example:444", ServerOrigin.parse("https://a.example:444/", false).serverUrl());
        assertEquals("https://a.example", ServerOrigin.parse("https://A.Example:443", false).serverUrl());
        assertEquals("http://[::1]:4400", ServerOrigin.parse("http://[::1]:4400/", true).serverUrl());
    }

    @Test
    public void aServerIsAnHttpsOriginAndNothingMore() {
        String[] refused = {
            "http://a.example/",
            "https://a.example/team",
            "https://a.example/?a=1",
            "https://a.example/#x",
            "https://ada:pw@a.example/",
            "ftp://a.example/",
            "a.example",
            "",
            null,
        };
        for (String text : refused) {
            assertNull(text, ServerOrigin.parse(text, false));
            assertNull(text, ServerOrigin.parse(text, true));
        }
    }

    @Test
    public void aServerOnLoopbackOverHttpPassesInADebugBuildOnly() {
        for (String text : new String[] { "http://127.0.0.1:4400/", "http://localhost:4400/", "http://[::1]:4400/" }) {
            assertNotNull(text, ServerOrigin.parse(text, true));
            assertNull(text, ServerOrigin.parse(text, false));
        }
        assertNull(ServerOrigin.parse("http://localhost.example.com/", true));
    }

    /** Capacitor keeps a main-frame navigation in the web view when its
     *  scheme and host match, whatever the port. Only the exact origin
     *  stays in the web view. */
    @Test
    public void aMainFrameNavigationToAnotherOriginOpensOutside() {
        assertTrue(server.opensOutside("https://a.example.evil.com/"));
        assertTrue(server.opensOutside("https://a.example:444/"));
        assertTrue(server.opensOutside("http://a.example/"));
        assertTrue(server.opensOutside("https://b.example/x"));
    }

    @Test
    public void aMainFrameNavigationOnTheServerOrNotOverHttpStaysWithCapacitor() {
        assertFalse(server.opensOutside("https://a.example/x"));
        assertFalse(server.opensOutside("https://A.Example:443/sign-in#abc"));
        assertFalse(server.opensOutside("about:blank"));
        assertFalse(server.opensOutside("data:text/html,x"));
        assertFalse(server.opensOutside("mailto:ada@example.com"));
        assertFalse(server.opensOutside("not a url"));
        assertFalse(server.opensOutside(null));
    }

    /** On the Connect screen the bridge shows the app's own origin. */
    @Test
    public void theConnectScreenKeepsItsOwnOriginOnly() {
        ServerOrigin connectScreen = ServerOrigin.parse("https://localhost", false);
        assertFalse(connectScreen.opensOutside("https://localhost/index.html"));
        assertTrue(connectScreen.opensOutside("https://a.example/"));
    }

    /** The bridge opens the first page as a path on the server URL. */
    @Test
    public void theFirstPageIsAPathOnTheOriginOfTheServer() {
        assertEquals("/", server.startPath("https://a.example/"));
        assertEquals("", server.startPath("https://a.example"));
        assertEquals("/sign-in#abc", server.startPath("https://a.example/sign-in#abc"));
        assertNull(server.startPath("https://b.example/sign-in#abc"));
        assertNull(server.startPath("https://a.example:444/sign-in#abc"));
        assertNull(server.startPath("http://a.example/sign-in#abc"));
        assertNull(server.startPath("https://ada@a.example/"));
        assertNull(server.startPath("not a url"));
        assertNull(server.startPath(null));
    }

    /** A tap on a Notification opens the path, the query and the fragment
     *  of a place on the server: the rule of {@code mobile/src/navigate.ts},
     *  with its cases. */
    @Test
    public void aTapOpensThePathQueryAndFragmentOfAPlaceOnTheServer() {
        assertEquals("/c/abc?x=1", server.place("https://a.example/c/abc?x=1"));
        assertEquals("/c/abc#card", server.place("https://a.example/c/abc#card"));
        assertEquals("/c/abc", server.place("https://A.Example:443/c/abc"));
        assertEquals("/", server.place("https://a.example"));
    }

    @Test
    public void aTapOnAPlaceOfAnotherOriginOpensTheRoot() {
        assertEquals("/", server.place("https://evil.example/c/abc"));
        assertEquals("/", server.place("http://a.example/c/abc"));
        assertEquals("/", server.place("https://a.example:8443/c/abc"));
    }

    @Test
    public void aTapOnAValueThatIsNotAnHttpUrlOpensTheRoot() {
        assertEquals("/", server.place("javascript:x"));
        assertEquals("/", server.place("not a url"));
        assertEquals("/", server.place("/c/abc"));
        assertEquals("/", server.place(null));
    }
}
