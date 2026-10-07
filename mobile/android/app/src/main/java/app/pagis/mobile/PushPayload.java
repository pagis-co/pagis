package app.pagis.mobile;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/**
 * The plaintext of a Notification: the Declarative Web Push JSON with the
 * Pagis fields in {@code notification.data} (ADR-0030). A field of the
 * wrong type is an error, and an unknown field is ignored.
 */
final class PushPayload {

    /** The Request that the Notification can answer. */
    static final class Request {
        final String id;
        final List<String> actions;

        Request(String id, List<String> actions) {
            this.id = id;
            this.actions = Collections.unmodifiableList(new ArrayList<>(actions));
        }
    }

    /** The {@code web_push} member of a Declarative Web Push message. */
    static final int DECLARATIVE_WEB_PUSH = 8030;
    /** The version of the payload format that this build reads. */
    static final int VERSION = 1;

    final String title;
    final String body;
    /** The absolute URL of the place of the item in the Product App. */
    final String navigate;
    /** The count of the Needs-You Queue, or null to leave the count. */
    final Integer badge;
    /** The id of the queue item. */
    final String item;
    /** A queue kind, or {@code test}. */
    final String kind;
    /** The Request, or null. */
    final Request request;

    PushPayload(String title, String body, String navigate, Integer badge, String item, String kind, Request request) {
        this.title = title;
        this.body = body;
        this.navigate = navigate;
        this.badge = badge;
        this.item = item;
        this.kind = kind;
        this.request = request;
    }

    /**
     * The payload in {@code json}. The members that name the format and its
     * version come first, because a later version can change every other
     * member.
     */
    static PushPayload parse(String json) throws PushPayloadException {
        JSONObject message;
        try {
            message = new JSONObject(json);
        } catch (JSONException ex) {
            throw new PushPayloadException(PushPayloadException.Reason.MALFORMED, "The payload is not a JSON object.");
        }
        JSONObject notification = object(message, "notification");
        JSONObject data = object(notification, "data");
        if (integer(message, "web_push") != DECLARATIVE_WEB_PUSH) {
            throw new PushPayloadException(PushPayloadException.Reason.NOT_DECLARATIVE_WEB_PUSH, "The payload is not a Declarative Web Push.");
        }
        int version = integer(data, "v");
        if (version != VERSION) {
            throw new PushPayloadException(PushPayloadException.Reason.UNKNOWN_VERSION, "This build does not read version " + version + " of the payload.");
        }
        return new PushPayload(
            string(notification, "title"),
            string(notification, "body"),
            string(notification, "navigate"),
            message.isNull("app_badge") ? null : integer(message, "app_badge"),
            string(data, "item"),
            string(data, "kind"),
            data.isNull("request") ? null : request(object(data, "request"))
        );
    }

    private static Request request(JSONObject request) throws PushPayloadException {
        Object actions = request.opt("actions");
        if (!(actions instanceof JSONArray)) throw badField("actions");
        JSONArray array = (JSONArray) actions;
        List<String> list = new ArrayList<>();
        for (int index = 0; index < array.length(); index++) {
            Object action = array.opt(index);
            if (!(action instanceof String)) throw badField("actions");
            list.add((String) action);
        }
        return new Request(string(request, "id"), list);
    }

    private static JSONObject object(JSONObject parent, String name) throws PushPayloadException {
        Object value = parent.opt(name);
        if (!(value instanceof JSONObject)) throw badField(name);
        return (JSONObject) value;
    }

    // The JSON library of Android coerces a number to a string in
    // getString, so each field checks its own type.
    private static String string(JSONObject parent, String name) throws PushPayloadException {
        Object value = parent.opt(name);
        if (!(value instanceof String)) throw badField(name);
        return (String) value;
    }

    private static int integer(JSONObject parent, String name) throws PushPayloadException {
        Object value = parent.opt(name);
        if (!(value instanceof Integer)) throw badField(name);
        return (Integer) value;
    }

    private static PushPayloadException badField(String name) {
        return new PushPayloadException(PushPayloadException.Reason.MALFORMED, "The payload has no " + name + " of the correct type.");
    }
}
