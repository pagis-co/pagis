package co.pagis.mobile;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

/**
 * The web view cannot load the server: the bridge opens the bundled
 * Unreachable screen, which names the server and the error.
 */
public class UnreachablePageTest {

    private final ServerOrigin server = ServerOrigin.parse("https://a.example", false);

    @Test
    public void theStartPathNamesTheServerAndTheError() {
        assertEquals(
            "/unreachable.html?server=https%3A%2F%2Fa.example&error=net%3A%3AERR_NAME_NOT_RESOLVED",
            UnreachablePage.startPath(server, "net::ERR_NAME_NOT_RESOLVED")
        );
    }

    /** The page reads the query with {@code URLSearchParams}, which reads
     *  a form query: {@code +} is a space. */
    @Test
    public void theQueryIsAFormQuery() {
        assertEquals(
            "/unreachable.html?server=https%3A%2F%2Fa.example&error=a%2Bb+%26+c%3Dd%3F",
            UnreachablePage.startPath(server, "a+b & c=d?")
        );
    }
}
