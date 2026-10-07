package co.pagis.mobile;

import android.Manifest;
import android.os.Build;
import androidx.core.app.NotificationManagerCompat;
import com.getcapacitor.JSObject;
import com.getcapacitor.PermissionState;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import com.getcapacitor.annotation.Permission;
import com.getcapacitor.annotation.PermissionCallback;
import com.google.android.gms.tasks.Tasks;
import com.google.firebase.messaging.FirebaseMessaging;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/**
 * {@code PagisPush}, the plugin of the app target that the Notifications
 * section of the Product App calls ({@code ui/src/push/pagisPush.ts}). The
 * Android System WebView has no Push API, so the app registers the Push
 * Subscription (ADR-0032).
 */
@CapacitorPlugin(
    name = "PagisPush",
    permissions = @Permission(strings = { Manifest.permission.POST_NOTIFICATIONS }, alias = PagisPushPlugin.NOTIFICATIONS)
)
public class PagisPushPlugin extends Plugin {

    static final String NOTIFICATIONS = "notifications";

    /** How long {@code subscribe} waits for the answer of the Person to the prompt. */
    private static final long PROMPT_MINUTES = 5;
    private static final long TOKEN_SECONDS = 30;

    /** The calls wait on the prompt and the network, so they run here, one at a time. */
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    /** The answer to the prompt that a {@code subscribe} waits for. */
    private volatile CompletableFuture<Boolean> promptAnswer;

    /** Whether the phone lets Pagis show notifications, as a Capacitor permission state. */
    @PluginMethod
    public void state(PluginCall call) {
        JSObject result = new JSObject();
        result.put("permission", permission().toString());
        call.resolve(result);
    }

    @PluginMethod
    public void subscribe(PluginCall call) {
        String vapidKey = call.getString("vapidKey");
        if (vapidKey == null || vapidKey.isEmpty()) {
            call.reject("subscribe needs the VAPID Key of the server.");
            return;
        }
        worker.execute(() -> {
            try {
                PushSubscription subscription = PushSubscriber.of(getContext()).subscribe(new Phone(call), vapidKey);
                JSObject keys = new JSObject();
                keys.put("p256dh", subscription.p256dh);
                keys.put("auth", subscription.auth);
                JSObject result = new JSObject();
                result.put("endpoint", subscription.endpoint);
                result.put("keys", keys);
                call.resolve(result);
            } catch (PushException ex) {
                call.reject(ex.getMessage());
            }
        });
    }

    @PluginMethod
    public void unsubscribe(PluginCall call) {
        worker.execute(() -> {
            try {
                PushSubscriber.of(getContext()).unsubscribe();
                call.resolve();
            } catch (PushException ex) {
                call.reject(ex.getMessage());
            }
        });
    }

    @Override
    protected void handleOnDestroy() {
        worker.shutdownNow();
    }

    /**
     * Before Android 13 there is no permission to ask for, and the Person
     * turns notifications off in the Settings app.
     */
    private PermissionState permission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return NotificationManagerCompat.from(getContext()).areNotificationsEnabled()
                ? PermissionState.GRANTED
                : PermissionState.DENIED;
        }
        return getPermissionState(NOTIFICATIONS);
    }

    @PermissionCallback
    private void notificationsAnswered(PluginCall call) {
        CompletableFuture<Boolean> answer = promptAnswer;
        promptAnswer = null;
        if (answer != null) answer.complete(getPermissionState(NOTIFICATIONS) == PermissionState.GRANTED);
    }

    /** The permission and the FCM token of this phone, for one {@code subscribe} call. */
    private final class Phone implements PushPlatform {

        private final PluginCall call;

        Phone(PluginCall call) {
            this.call = call;
        }

        @Override
        public boolean askPermission() throws PushException {
            PermissionState state = permission();
            if (state == PermissionState.GRANTED) return true;
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return false;
            CompletableFuture<Boolean> answer = new CompletableFuture<>();
            promptAnswer = answer;
            getActivity().runOnUiThread(() -> requestPermissionForAlias(NOTIFICATIONS, call, "notificationsAnswered"));
            try {
                return answer.get(PROMPT_MINUTES, TimeUnit.MINUTES);
            } catch (InterruptedException ex) {
                Thread.currentThread().interrupt();
                throw new PushException("Pagis stopped while it asked to show notifications.", ex);
            } catch (ExecutionException | TimeoutException ex) {
                throw new PushException("Pagis got no answer when it asked to show notifications.", ex);
            }
        }

        @Override
        public String token() throws PushException {
            try {
                return Tasks.await(FirebaseMessaging.getInstance().getToken(), TOKEN_SECONDS, TimeUnit.SECONDS);
            } catch (InterruptedException ex) {
                Thread.currentThread().interrupt();
                throw new PushException("Pagis stopped while it got its token from Firebase.", ex);
            } catch (ExecutionException | TimeoutException ex) {
                throw new PushException("Pagis did not get its token from Firebase: " + ex.getMessage(), ex);
            }
        }
    }
}
