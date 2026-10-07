package app.pagis.mobile;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

/**
 * The secret items of the push: files that the Tink AEAD of the app
 * encrypts, with the name of each item as the associated data.
 */
public class SealedFilesTest {

    @Rule
    public final TemporaryFolder folder = new TemporaryFolder();

    private File directory;
    private SealedFiles files;

    @Before
    public void makeFiles() throws Exception {
        directory = new File(folder.getRoot(), "push");
        files = new SealedFiles(directory, SessionCopyTest.testAead());
    }

    @Test
    public void anItemReadsBackWhatWasWritten() throws Exception {
        files.write("a", new byte[] { 1, 2, 3 });
        files.write("a", new byte[] { 4, 5 });

        assertArrayEquals(new byte[] { 4, 5 }, files.read("a"));
    }

    @Test
    public void theFileHoldsNoClearText() throws Exception {
        files.write("a", "the-secret".getBytes(StandardCharsets.UTF_8));

        String bytes = new String(Files.readAllBytes(new File(directory, "a").toPath()), StandardCharsets.ISO_8859_1);
        assertFalse(bytes.contains("the-secret"));
    }

    /** The name is the associated data, so a file under another name does not decrypt. */
    @Test
    public void aFileUnderAnotherNameReadsAsNone() throws Exception {
        files.write("a", new byte[] { 1 });
        Files.copy(new File(directory, "a").toPath(), new File(directory, "b").toPath());

        assertNull(files.read("b"));
    }

    @Test
    public void aDeletedItemReadsAsNone() throws Exception {
        files.write("a", new byte[] { 1 });
        files.delete("a");
        files.delete("a");

        assertNull(files.read("a"));
        assertNull(files.read("never-written"));
    }
}
