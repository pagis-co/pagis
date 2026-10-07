package app.pagis.mobile;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Secret items in memory. It writes each change to the steps of a test. */
final class MemorySecrets implements SecretStore {

    final Map<String, byte[]> values = new HashMap<>();
    private final List<String> steps;

    MemorySecrets(List<String> steps) {
        this.steps = steps;
    }

    @Override
    public byte[] read(String name) {
        return values.get(name);
    }

    @Override
    public void write(String name, byte[] data) {
        steps.add("write " + name);
        values.put(name, data.clone());
    }

    @Override
    public void delete(String name) {
        steps.add("delete " + name);
        values.remove(name);
    }
}
