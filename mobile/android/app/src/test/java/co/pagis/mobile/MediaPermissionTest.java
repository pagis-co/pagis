package co.pagis.mobile;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.webkit.PermissionRequest;
import org.junit.Test;

/**
 * The Mobile App grants the microphone to its own server alone. A page of
 * another origin gets no microphone, and no page gets the camera
 * (ADR-0032). A request of the web view names an origin and no frame.
 */
public class MediaPermissionTest {

    private static final String AUDIO = PermissionRequest.RESOURCE_AUDIO_CAPTURE;
    private static final String VIDEO = PermissionRequest.RESOURCE_VIDEO_CAPTURE;

    private final ServerOrigin server = ServerOrigin.parse("https://a.example", false);

    @Test
    public void theServerGetsTheMicrophone() {
        assertTrue(MediaPermission.grantsMicrophone(server, "https://a.example/", new String[] { AUDIO }));
        assertTrue(MediaPermission.grantsMicrophone(server, "https://a.example", new String[] { AUDIO }));
    }

    @Test
    public void anotherOriginGetsNoMicrophone() {
        for (String origin : new String[] {
            "https://b.example/", "https://a.example:444/", "http://a.example/", "https://x.a.example/", "null", null,
        }) {
            assertFalse(origin, MediaPermission.grantsMicrophone(server, origin, new String[] { AUDIO }));
        }
    }

    @Test
    public void noPageGetsTheCamera() {
        assertFalse(MediaPermission.grantsMicrophone(server, "https://a.example/", new String[] { VIDEO }));
        assertFalse(MediaPermission.grantsMicrophone(server, "https://a.example/", new String[] { AUDIO, VIDEO }));
        assertFalse(MediaPermission.grantsMicrophone(
            server, "https://a.example/", new String[] { PermissionRequest.RESOURCE_PROTECTED_MEDIA_ID }
        ));
        assertFalse(MediaPermission.grantsMicrophone(server, "https://a.example/", new String[0]));
        assertFalse(MediaPermission.grantsMicrophone(server, "https://a.example/", null));
    }

    /** The Connect screen shows no server, and needs no microphone. */
    @Test
    public void theConnectScreenGetsNoMicrophone() {
        assertFalse(MediaPermission.grantsMicrophone(null, "https://localhost/", new String[] { AUDIO }));
    }
}
