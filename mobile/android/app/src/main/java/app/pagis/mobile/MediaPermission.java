package app.pagis.mobile;

import android.webkit.PermissionRequest;

/**
 * Which request of a page for the microphone or the camera the app grants
 * (ADR-0032): the microphone, to the stored server alone. Every other
 * resource and every other origin is denied.
 */
final class MediaPermission {

    private MediaPermission() {}

    /**
     * Whether the app grants a request of {@code origin} for {@code resources}.
     *
     * @param server The stored server, or null on the Connect screen.
     */
    static boolean grantsMicrophone(ServerOrigin server, String origin, String[] resources) {
        return server != null
            && server.matches(origin)
            && resources != null
            && resources.length == 1
            && PermissionRequest.RESOURCE_AUDIO_CAPTURE.equals(resources[0]);
    }
}
