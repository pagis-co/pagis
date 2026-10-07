package app.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;

import java.io.File;
import java.io.IOException;
import java.net.CookieHandler;
import java.net.URI;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import okhttp3.mockwebserver.Dispatcher;
import okhttp3.mockwebserver.MockResponse;
import okhttp3.mockwebserver.MockWebServer;
import okhttp3.mockwebserver.RecordedRequest;
import okhttp3.mockwebserver.SocketPolicy;
import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/**
 * When the app comes to the foreground, it reads the Needs-You Queue with
 * the copy of the Session and cancels each Notification whose item left
 * the queue (ADR-0032). {@link MockWebServer} answers for the daemon, and
 * a fake stands in for the notification manager.
 */
public class StaleNotificationsTest {

    @Rule
    public final TemporaryFolder folder = new TemporaryFolder();

    private final MockWebServer daemon = new MockWebServer();
    /** The steps of a test, in order. The daemon writes to it from its own thread. */
    private final List<String> steps = Collections.synchronizedList(new ArrayList<>());
    private final List<RecordedRequest> requests = Collections.synchronizedList(new ArrayList<>());
    private final FakeTray tray = new FakeTray();
    private MockResponse answer = queue("request:r-1", "call:c-1");
    private ServerOrigin server;
    private SessionCopy copy;
    /** The failure of the last clean-up, or null. */
    private NeedsYouException failure;

    @Before
    public void start() throws Exception {
        daemon.setDispatcher(new Dispatcher() {
            @Override
            public MockResponse dispatch(RecordedRequest request) {
                requests.add(request);
                steps.add(request.getMethod() + " " + request.getPath());
                return answer;
            }
        });
        daemon.start();
        // A debug build takes `http://` on a loopback host, where the test
        // daemon answers.
        server = ServerOrigin.parse("http://localhost:" + daemon.getPort(), true);
        copy = new SessionCopy(new File(folder.getRoot(), "session"), SessionCopyTest.testAead());
        copy.write(server, new Session("s1", 1_900_000_000_000L));
        tray.shown.addAll(Arrays.asList(
            new NotificationTray.Shown("request:r-1", 1),
            new NotificationTray.Shown("run:old", 1),
            new NotificationTray.Shown(null, 2)
        ));
    }

    @After
    public void stop() throws IOException {
        daemon.shutdown();
    }

    /** The read has no {@code Origin}, so the cross-origin check of the
     *  daemon passes it to the Session check as a request from a program. */
    @Test
    public void theReadSendsTheCopyOfTheSessionAndNoOrigin() {
        clean();

        assertNull(failure);
        assertEquals(1, requests.size());
        RecordedRequest sent = requests.get(0);
        assertEquals("GET", sent.getMethod());
        assertEquals("/api/v1/needs-you", sent.getPath());
        assertEquals("pagis_session=s1", sent.getHeader("Cookie"));
        assertNull(sent.getHeader("Origin"));
        assertNull(sent.getHeader("Sec-Fetch-Site"));
    }

    /**
     * Capacitor sets the cookie store of the web view as the default
     * {@link CookieHandler} of the process. The read does not use it, so it
     * sends the copy of the Session alone.
     */
    @Test
    public void theDefaultCookieHandlerOfTheProcessIsNotRead() {
        CookieHandler before = CookieHandler.getDefault();
        CookieHandler.setDefault(new CookieHandler() {
            @Override
            public Map<String, List<String>> get(URI uri, Map<String, List<String>> headers) {
                return Collections.singletonMap("Cookie", Collections.singletonList("pagis_session=web"));
            }

            @Override
            public void put(URI uri, Map<String, List<String>> headers) {}
        });
        try {
            clean();
        } finally {
            CookieHandler.setDefault(before);
        }

        assertNull(failure);
        assertEquals(Collections.singletonList("pagis_session=s1"), requests.get(0).getHeaders().values("Cookie"));
    }

    /**
     * A Notification with no item, such as the fallback, names no item of
     * the queue, so it goes too. The app lists the Notifications before it
     * reads the queue: a Notification that arrives during the read is not
     * in the list, so it stays, because its item can be newer than the
     * answer. Android shows the count of the Notifications, so the app sets
     * no badge.
     */
    @Test
    public void itCancelsEachNotificationWhoseItemIsNotInTheQueue() {
        clean();

        assertEquals(Arrays.asList("list", "GET /api/v1/needs-you", "cancel run:old 1", "cancel null 2"), steps);
    }

    @Test
    public void anEmptyQueueCancelsEveryNotification() {
        answer = queue();

        clean();

        assertEquals(
            Arrays.asList("list", "GET /api/v1/needs-you", "cancel request:r-1 1", "cancel run:old 1", "cancel null 2"),
            steps
        );
    }

    @Test
    public void aFailedReadCancelsNothing() {
        for (MockResponse failed : Arrays.asList(
            new MockResponse().setResponseCode(500).setBody("{}"),
            new MockResponse().setResponseCode(404).setBody("{}"),
            new MockResponse().setSocketPolicy(SocketPolicy.DISCONNECT_AT_START),
            new MockResponse().setBody("not json"),
            new MockResponse().setBody("{\"items\": [{\"id\": 7}], \"count\": 1}"),
            new MockResponse().setBody("{\"items\": []}")
        )) {
            answer = failed;
            steps.clear();

            clean();

            assertNotNull(failed.toString(), failure);
            assertEquals(failed.toString(), Arrays.asList("list", "GET /api/v1/needs-you"), steps);
            assertEquals(new Session("s1", 1_900_000_000_000L), copy.read(server));
        }
    }

    /** A {@code 401} ends the Session: the app deletes the copy, as each
     *  native request does, and cancels nothing. */
    @Test
    public void a401DeletesTheCopyAndCancelsNothing() {
        answer = new MockResponse().setResponseCode(401).setBody("{}");

        clean();

        assertEquals(401, failure.status);
        assertEquals(Arrays.asList("list", "GET /api/v1/needs-you"), steps);
        assertNull(copy.read(server));
    }

    @Test
    public void noCopyOfTheSessionReadsNothing() {
        copy.delete();

        clean();

        assertEquals(Collections.emptyList(), steps);
    }

    private void clean() {
        failure = null;
        try {
            new StaleNotifications(new NeedsYouClient(), copy, tray).clean(server);
        } catch (NeedsYouException ex) {
            failure = ex;
        }
    }

    /** A queue with these item ids, and their count. */
    private static MockResponse queue(String... ids) {
        StringBuilder items = new StringBuilder();
        for (String id : ids) {
            if (items.length() > 0) items.append(", ");
            items.append("{\"kind\": \"failed\", \"id\": \"").append(id).append("\"}");
        }
        return new MockResponse()
            .setHeader("Content-Type", "application/json")
            .setBody("{\"items\": [" + items + "], \"count\": " + ids.length + "}");
    }

    /** The Notifications of the app. It writes each change to the steps. */
    private final class FakeTray implements NotificationTray {
        final List<Shown> shown = new ArrayList<>();

        @Override
        public List<Shown> shown() {
            steps.add("list");
            return new ArrayList<>(shown);
        }

        @Override
        public void cancel(String tag, int id) {
            steps.add("cancel " + tag + " " + id);
        }
    }
}
