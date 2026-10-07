package co.pagis.mobile;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.json.JSONObject;

/**
 * {@code fixtures/web-push.json} of the repository: the keys of a Push
 * Subscription, one body that {@code pagis-push} encrypted for it, and its
 * plaintext. The test task gives its path in the system property
 * {@code pagis.webPushFixture} ({@code app/build.gradle}).
 */
final class WebPushFixture {

    final PushKeys keys;
    final byte[] body;
    final String plaintext;

    private WebPushFixture(PushKeys keys, byte[] body, String plaintext) {
        this.keys = keys;
        this.body = body;
        this.plaintext = plaintext;
    }

    static WebPushFixture load() throws Exception {
        String path = System.getProperty("pagis.webPushFixture");
        if (path == null) throw new IllegalStateException("The test task gives no pagis.webPushFixture.");
        JSONObject file = new JSONObject(new String(Files.readAllBytes(new File(path).toPath()), StandardCharsets.UTF_8));
        JSONObject subscription = file.getJSONObject("subscription");
        PushKeys keys = new PushKeys(
            RFC8291Vector.pkcs8(RFC8291Vector.data(subscription.getString("private_key"))),
            RFC8291Vector.data(subscription.getString("p256dh")),
            RFC8291Vector.data(subscription.getString("auth"))
        );
        return new WebPushFixture(keys, RFC8291Vector.data(file.getString("body")), file.getString("plaintext"));
    }
}
