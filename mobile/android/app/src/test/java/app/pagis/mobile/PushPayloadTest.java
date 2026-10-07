package app.pagis.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;

import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.Map;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.junit.Test;

/** {@link PushPayload} reads the Declarative Web Push JSON of ADR-0030. */
public class PushPayloadTest {

    @Test
    public void anApprovalGivesEachField() throws Exception {
        PushPayload payload = PushPayload.parse(approval(message -> {}));

        assertEquals("Robin", payload.title);
        assertEquals("Robin needs your approval\nhost_shell", payload.body);
        assertEquals("https://pagis.example.com/c/ch-1", payload.navigate);
        assertEquals(Integer.valueOf(3), payload.badge);
        assertEquals("request:r-1", payload.item);
        assertEquals("approval", payload.kind);
        assertEquals("r-1", payload.request.id);
        assertEquals(Arrays.asList("approve_once", "deny"), payload.request.actions);
    }

    @Test
    public void thePayloadOfTheTestNotificationHasNoBadgeAndNoRequest() throws Exception {
        String json = new JSONObject()
            .put("web_push", 8030)
            .put("notification", new JSONObject()
                .put("title", "Pagis")
                .put("body", "Notifications work here.")
                .put("navigate", "https://pagis.example.com/settings/notifications")
                .put("data", new JSONObject().put("v", 1).put("item", "test").put("kind", "test")))
            .put("mutable", true)
            .toString();

        PushPayload payload = PushPayload.parse(json);

        assertNull(payload.badge);
        assertNull(payload.request);
        assertEquals("test", payload.kind);
        assertEquals("test", payload.item);
    }

    @Test
    public void anUnknownFieldIsIgnored() throws Exception {
        PushPayload payload = PushPayload.parse(approval(message -> {
            message.put("lang", "en");
            notification(message).put("silent", false);
            data(message).put("later", new JSONObject().put("a", 1));
        }));

        assertEquals("Robin", payload.title);
    }

    @Test
    public void aFieldOfTheWrongTypeIsAnError() {
        Map<String, Change> changes = new LinkedHashMap<>();
        changes.put("title", message -> notification(message).put("title", 42));
        changes.put("body", message -> notification(message).put("body", new JSONArray().put("text")));
        changes.put("navigate", message -> notification(message).put("navigate", false));
        changes.put("app_badge", message -> message.put("app_badge", "3"));
        changes.put("item", message -> data(message).put("item", 1));
        changes.put("kind", message -> data(message).put("kind", JSONObject.NULL));
        changes.put("request", message -> data(message).put("request", "r-1"));
        changes.put("actions", message -> data(message).getJSONObject("request").put("actions", "approve_once"));
        changes.put("an action", message -> data(message).getJSONObject("request").put("actions", new JSONArray().put(1)));
        changes.put("v", message -> data(message).put("v", "1"));
        for (Map.Entry<String, Change> change : changes.entrySet()) {
            String json = approval(change.getValue());
            PushPayloadException error = assertThrows(change.getKey(), PushPayloadException.class, () -> PushPayload.parse(json));
            assertEquals(change.getKey(), PushPayloadException.Reason.MALFORMED, error.reason);
        }
    }

    @Test
    public void aMissingFieldIsAnError() {
        Map<String, Change> changes = new LinkedHashMap<>();
        changes.put("title", message -> notification(message).remove("title"));
        changes.put("navigate", message -> notification(message).remove("navigate"));
        changes.put("data", message -> notification(message).remove("data"));
        changes.put("notification", message -> message.remove("notification"));
        for (Map.Entry<String, Change> change : changes.entrySet()) {
            String json = approval(change.getValue());
            assertThrows(change.getKey(), PushPayloadException.class, () -> PushPayload.parse(json));
        }
    }

    @Test
    public void anotherVersionIsAnError() {
        String json = approval(message -> data(message).put("v", 2));

        PushPayloadException error = assertThrows(PushPayloadException.class, () -> PushPayload.parse(json));

        assertEquals(PushPayloadException.Reason.UNKNOWN_VERSION, error.reason);
    }

    @Test
    public void aMessageThatIsNotADeclarativeWebPushIsAnError() {
        String json = approval(message -> message.put("web_push", 8291));

        PushPayloadException error = assertThrows(PushPayloadException.class, () -> PushPayload.parse(json));

        assertEquals(PushPayloadException.Reason.NOT_DECLARATIVE_WEB_PUSH, error.reason);
        assertThrows(PushPayloadException.class, () -> PushPayload.parse("not json"));
    }

    /** A change to the JSON of a payload. Android declares {@link JSONException} as checked. */
    interface Change {
        void apply(JSONObject message) throws JSONException;
    }

    /** The JSON of a tool action Approval as the daemon makes it, after {@code change}. */
    static String approval(Change change) {
        try {
            JSONObject message = new JSONObject()
                .put("web_push", 8030)
                .put("notification", new JSONObject()
                    .put("title", "Robin")
                    .put("body", "Robin needs your approval\nhost_shell")
                    .put("navigate", "https://pagis.example.com/c/ch-1")
                    .put("data", new JSONObject()
                        .put("v", 1)
                        .put("item", "request:r-1")
                        .put("kind", "approval")
                        .put("request", new JSONObject()
                            .put("id", "r-1")
                            .put("actions", new JSONArray().put("approve_once").put("deny")))))
                .put("app_badge", 3)
                .put("mutable", true);
            change.apply(message);
            return message.toString();
        } catch (JSONException ex) {
            throw new IllegalStateException(ex);
        }
    }

    static JSONObject notification(JSONObject message) throws JSONException {
        return message.getJSONObject("notification");
    }

    static JSONObject data(JSONObject message) throws JSONException {
        return notification(message).getJSONObject("data");
    }
}
