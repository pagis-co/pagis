package app.pagis.mobile;

import android.Manifest;
import android.content.pm.PackageManager;
import android.webkit.PermissionRequest;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;
import com.getcapacitor.Bridge;
import com.getcapacitor.BridgeWebChromeClient;

/**
 * The chrome client of the bridge.
 *
 * Capacitor grants each resource that a page asks for after the permission
 * of the system, also the camera, to each origin. This client grants the
 * microphone to the stored server alone, after the app holds
 * {@code RECORD_AUDIO}, and denies every other request ({@link MediaPermission}).
 * The Person sees the prompt of the system alone, and only at the first
 * request.
 */
class PagisChromeClient extends BridgeWebChromeClient {

    private final Bridge bridge;
    private final ServerOrigin server;
    private final ActivityResultLauncher<String> microphone;
    /** The request that waits for the answer of the Person to the prompt. */
    private PermissionRequest waiting;

    /**
     * Make the client while the activity is created, because it registers
     * for the result of the permission prompt.
     *
     * @param server The stored server, or null on the Connect screen.
     */
    PagisChromeClient(Bridge bridge, ServerOrigin server) {
        super(bridge);
        this.bridge = bridge;
        this.server = server;
        this.microphone = bridge.registerForActivityResult(
            new ActivityResultContracts.RequestPermission(),
            this::microphoneAnswered
        );
    }

    @Override
    public void onPermissionRequest(PermissionRequest request) {
        if (!MediaPermission.grantsMicrophone(server, request.getOrigin().toString(), request.getResources())) {
            request.deny();
            return;
        }
        if (holdsMicrophone()) {
            request.grant(new String[] { PermissionRequest.RESOURCE_AUDIO_CAPTURE });
            return;
        }
        if (waiting != null) waiting.deny();
        waiting = request;
        microphone.launch(Manifest.permission.RECORD_AUDIO);
    }

    @Override
    public void onPermissionRequestCanceled(PermissionRequest request) {
        if (request == waiting) waiting = null;
        super.onPermissionRequestCanceled(request);
    }

    private boolean holdsMicrophone() {
        return ContextCompat.checkSelfPermission(bridge.getContext(), Manifest.permission.RECORD_AUDIO)
            == PackageManager.PERMISSION_GRANTED;
    }

    private void microphoneAnswered(Boolean granted) {
        PermissionRequest request = waiting;
        waiting = null;
        if (request == null) return;
        if (Boolean.TRUE.equals(granted)) {
            request.grant(new String[] { PermissionRequest.RESOURCE_AUDIO_CAPTURE });
        } else {
            request.deny();
        }
    }
}
