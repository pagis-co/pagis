package co.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.Collections;
import java.util.List;
import okhttp3.mockwebserver.Dispatcher;
import okhttp3.mockwebserver.MockResponse;
import okhttp3.mockwebserver.MockWebServer;
import okhttp3.mockwebserver.RecordedRequest;
import org.json.JSONObject;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;

/**
 * {@code PagisPush} registers a Push Subscription through the Push Relay
 * (ADR-0032). A fake platform gives the permission and the FCM token,
 * {@link MockWebServer} answers for the Push Relay, and the secret items
 * are in memory.
 */
public class PushSubscriberTest {

    /** The VAPID Key of the server: an uncompressed P-256 point. */
    private static final String VAPID_KEY = point((byte) 1);
    private static final String OTHER_VAPID_KEY = point((byte) 9);

    @Rule
    public final MockWebServer relay = new MockWebServer();

    /** The steps of a test, in order. The relay writes to it from its own thread. */
    private final List<String> steps = Collections.synchronizedList(new ArrayList<>());
    /** The requests that the relay got, in order. */
    private final List<Request> requests = Collections.synchronizedList(new ArrayList<>());
    private final MemorySecrets items = new MemorySecrets(steps);
    private final FakePlatform platform = new FakePlatform();
    private int postStatus = 201;
    private int putStatus = 204;
    private PushSubscriber subscriber;

    @Before
    public void startRelay() {
        relay.setDispatcher(new Dispatcher() {
            @Override
            public MockResponse dispatch(RecordedRequest request) {
                Request seen = new Request(
                    request.getMethod() + " " + request.getPath(),
                    request.getHeader("Authorization"),
                    request.getBody().readUtf8()
                );
                requests.add(seen);
                steps.add(seen.line);
                switch (request.getMethod()) {
                    case "POST":
                        int n = (int) requests.stream().filter(r -> r.line.startsWith("POST")).count();
                        return new MockResponse()
                            .setResponseCode(postStatus)
                            .setBody("{\"id\":\"id-" + n + "\",\"secret\":\"secret-" + n
                                + "\",\"endpoint\":\"https://relay.example/v1/push/id-" + n + "\"}");
                    case "PUT":
                        return new MockResponse().setResponseCode(putStatus);
                    default:
                        return new MockResponse().setResponseCode(204);
                }
            }
        });
        String origin = relay.url("/").toString();
        subscriber = subscriberFor(new RelayClient(origin.substring(0, origin.length() - 1)));
    }

    @Test
    public void subscribeAsksThenRegistersThenMakesTheKeys() throws Exception {
        PushSubscription subscription = subscriber.subscribe(platform, VAPID_KEY);

        assertEquals(Arrays.asList(
            "permission",
            "token",
            "POST /v1/registrations",
            "write " + RegistrationStore.ITEM,
            "write " + PushKeyStore.ITEM
        ), steps);
        JSONObject body = new JSONObject(requests.get(0).body);
        assertEquals("android", body.getString("platform"));
        assertFalse(body.has("environment"));
        assertEquals("token-1", body.getString("token"));
        assertEquals(VAPID_KEY, body.getString("vapid_key"));
        assertEquals(3, body.length());

        PushKeys keys = new PushKeyStore(items, new SecureRandom()).keys();
        assertEquals("https://relay.example/v1/push/id-1", subscription.endpoint);
        assertEquals(keys.p256dh(), subscription.p256dh);
        assertEquals(keys.authText(), subscription.auth);
    }

    @Test
    public void aSecondSubscribeWithTheSameKeyRegistersNothing() throws Exception {
        PushSubscription first = subscriber.subscribe(platform, VAPID_KEY);

        PushSubscription second = subscriber.subscribe(platform, VAPID_KEY);

        assertEquals(first, second);
        assertEquals(Collections.singletonList("POST /v1/registrations"), lines());
    }

    @Test
    public void subscribeWithAnotherKeyRemovesTheOldRegistrationFirst() throws Exception {
        PushSubscription first = subscriber.subscribe(platform, VAPID_KEY);

        PushSubscription second = subscriber.subscribe(platform, OTHER_VAPID_KEY);

        assertEquals(Arrays.asList(
            "POST /v1/registrations",
            "DELETE /v1/registrations/id-1",
            "POST /v1/registrations"
        ), lines());
        assertEquals("https://relay.example/v1/push/id-2", second.endpoint);
        assertNotEquals(first.p256dh, second.p256dh);
    }

    @Test
    public void aNewTokenSendsOnePutToTheRelay() throws Exception {
        subscriber.subscribe(platform, VAPID_KEY);

        subscriber.tokenChanged("token-2");
        subscriber.tokenChanged("token-2");

        assertEquals(Arrays.asList("POST /v1/registrations", "PUT /v1/registrations/id-1"), lines());
        Request put = requests.get(1);
        assertEquals("Bearer secret-1", put.authorization);
        JSONObject body = new JSONObject(put.body);
        assertEquals("token-2", body.getString("token"));
        assertEquals(1, body.length());
        assertEquals("token-2", new RegistrationStore(items).read().token);
    }

    /** The endpoint stays the same, so the Push Subscription of the daemon stays the same. */
    @Test
    public void subscribeSendsATokenThatChangedAndKeepsTheEndpoint() throws Exception {
        PushSubscription first = subscriber.subscribe(platform, VAPID_KEY);
        platform.nextToken = "token-2";

        PushSubscription second = subscriber.subscribe(platform, VAPID_KEY);

        assertEquals(Arrays.asList("POST /v1/registrations", "PUT /v1/registrations/id-1"), lines());
        assertEquals(first, second);
    }

    /**
     * The relay removes a registration when FCM says that its token is
     * gone. The app then forgets it, and the next subscribe registers again.
     */
    @Test
    public void aTokenThatTheRelayDoesNotKnowForgetsTheRegistration() throws Exception {
        subscriber.subscribe(platform, VAPID_KEY);
        putStatus = 404;

        subscriber.tokenChanged("token-2");

        assertNull(new RegistrationStore(items).read());
        assertNull(items.read(PushKeyStore.ITEM));
    }

    @Test
    public void unsubscribeDeletesTheRegistrationThenTheKeys() throws Exception {
        subscriber.subscribe(platform, VAPID_KEY);
        steps.clear();

        subscriber.unsubscribe();

        assertEquals(Arrays.asList(
            "DELETE /v1/registrations/id-1",
            "delete " + RegistrationStore.ITEM,
            "delete " + PushKeyStore.ITEM
        ), steps);
        assertEquals("Bearer secret-1", requests.get(1).authorization);
        assertNull(new RegistrationStore(items).read());
        assertNull(items.read(PushKeyStore.ITEM));
    }

    @Test
    public void aRefusedPermissionRegistersNothing() {
        platform.allows = false;

        try {
            subscriber.subscribe(platform, VAPID_KEY);
            fail("subscribe did not fail");
        } catch (PushException expected) {
            // The Person did not allow notifications.
        }
        assertEquals(Collections.singletonList("permission"), steps);
    }

    @Test
    public void aRefusedRegistrationKeepsNothing() {
        postStatus = 422;

        try {
            subscriber.subscribe(platform, VAPID_KEY);
            fail("subscribe did not fail");
        } catch (PushException expected) {
            assertTrue(expected.getMessage(), expected.getMessage().contains("422"));
        }
        assertTrue(items.values.isEmpty());
    }

    // --- helpers ---

    private PushSubscriber subscriberFor(RelayClient client) {
        return new PushSubscriber(client, new RegistrationStore(items), new PushKeyStore(items, new SecureRandom()));
    }

    private List<String> lines() {
        List<String> lines = new ArrayList<>();
        synchronized (requests) {
            for (Request request : requests) lines.add(request.line);
        }
        return lines;
    }

    /** An uncompressed point as base64url: {@code 0x04} and 64 bytes. */
    private static String point(byte fill) {
        byte[] bytes = new byte[65];
        Arrays.fill(bytes, fill);
        bytes[0] = 4;
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bytes);
    }

    /** One request that the relay got. */
    private static final class Request {
        final String line;
        final String authorization;
        final String body;

        Request(String line, String authorization, String body) {
            this.line = line;
            this.authorization = authorization;
            this.body = body;
        }
    }

    /** The permission and the FCM token of a phone. */
    private final class FakePlatform implements PushPlatform {
        boolean allows = true;
        String nextToken = "token-1";

        @Override
        public boolean askPermission() {
            steps.add("permission");
            return allows;
        }

        @Override
        public String token() {
            steps.add("token");
            return nextToken;
        }
    }
}
