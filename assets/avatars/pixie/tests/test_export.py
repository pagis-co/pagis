"""Check the file that a glTF client receives."""

import json
import math
from pathlib import Path
import struct
import unittest


ASSET = Path(__file__).resolve().parents[1] / 'exports' / 'pixie.glb'


def read_glb(path=ASSET):
    data = path.read_bytes()
    magic, version, length = struct.unpack_from('<4sII', data)
    assert magic == b'glTF' and version == 2 and length == len(data)
    size, kind = struct.unpack_from('<II', data, 12)
    assert kind == 0x4E4F534A
    document = json.loads(data[20:20 + size])
    offset = 20 + size
    bin_size, bin_kind = struct.unpack_from('<II', data, offset)
    assert bin_kind == 0x004E4942
    return document, data[offset + 8:offset + 8 + bin_size]


def floats(doc, binary, index):
    accessor = doc['accessors'][index]
    view = doc['bufferViews'][accessor['bufferView']]
    assert accessor['componentType'] == 5126
    width = {'SCALAR': 1, 'VEC3': 3, 'VEC4': 4, 'MAT4': 16}[accessor['type']]
    start = view.get('byteOffset', 0) + accessor.get('byteOffset', 0)
    stride = view.get('byteStride', width * 4)
    return [struct.unpack_from('<' + 'f' * width, binary, start + row * stride)
            for row in range(accessor['count'])]


class PixieExportTest(unittest.TestCase):
    def test_character_loads_from_one_file_with_a_working_skin(self):
        doc, binary = read_glb()
        self.assertEqual(doc['asset']['version'], '2.0')
        self.assertTrue(doc['meshes'])
        self.assertTrue(doc['skins'])
        self.assertTrue(any('skin' in node for node in doc['nodes']))
        self.assertNotIn('uri', doc['buffers'][0])
        self.assertGreater(len(binary), 1000)
        names = {node.get('name') for node in doc['nodes']}
        self.assertTrue({'Root', 'Body', 'Head', 'Ear.L', 'Ear.R'} <= names)
        for mesh in doc['meshes']:
            for part in mesh['primitives']:
                self.assertIn('POSITION', part['attributes'])
                position = doc['accessors'][part['attributes']['POSITION']]
                self.assertGreater(position['count'], 2)
        self.assertLess(ASSET.stat().st_size, 8_000_000)

    def test_resting_hands_have_no_arm_or_hand_controls(self):
        doc, _ = read_glb()
        names = {node.get('name', '') for node in doc['nodes']}
        self.assertFalse(any(name.startswith(('Arm.', 'Hand.', 'Socket_Hand')) for name in names))
        self.assertEqual(len(doc['skins'][0]['joints']), 8)

    def test_motion_is_limited_to_the_face_ears_and_whole_avatar(self):
        doc, binary = read_glb()
        moving = {}
        for clip in doc['animations']:
            moved = set()
            for channel in clip['channels']:
                sampler = clip['samplers'][channel['sampler']]
                values = floats(doc, binary, sampler['output'])
                if any(any(abs(a-b) > .00001 for a,b in zip(row, values[0])) for row in values[1:]):
                    moved.add(doc['nodes'][channel['target']['node']]['name'])
            self.assertTrue(moved <= {'Root', 'Head', 'Ear.L', 'Ear.R'}, (clip['name'], moved))
            moving[clip['name']] = moved
        self.assertIn('Root', moving['Celebrate'])

    def test_celebration_makes_one_full_turn(self):
        doc, binary = read_glb()
        clip = next(clip for clip in doc['animations'] if clip['name'] == 'Celebrate')
        rotation = next(channel for channel in clip['channels']
                        if channel['target']['path'] == 'rotation'
                        and doc['nodes'][channel['target']['node']]['name'] == 'Root')
        sampler = clip['samplers'][rotation['sampler']]
        values = floats(doc, binary, sampler['output'])
        angle = sum(2 * math.acos(min(1, abs(sum(a*b for a,b in zip(left,right)))))
                    for left,right in zip(values,values[1:]))
        self.assertAlmostEqual(angle, math.tau, delta=.01)

    def test_surface_texture_is_embedded_and_has_uv_coordinates(self):
        doc, binary = read_glb()
        self.assertEqual(len(doc.get('images', [])), 1)
        image = doc['images'][0]
        self.assertNotIn('uri', image)
        self.assertEqual(image['mimeType'], 'image/png')
        view = doc['bufferViews'][image['bufferView']]
        start = view.get('byteOffset', 0)
        self.assertEqual(binary[start:start + 8], b'\x89PNG\r\n\x1a\n')
        self.assertEqual(struct.unpack_from('>II', binary, start + 16), (512, 512))
        textured = 0
        for mesh in doc['meshes']:
            for part in mesh['primitives']:
                if 'normalTexture' in doc['materials'][part['material']]:
                    self.assertIn('TEXCOORD_0', part['attributes'])
                    self.assertIn('TANGENT', part['attributes'])
                    textured += 1
        self.assertGreater(textured, 3)

    def test_painted_colors_stay_inside_the_gltf_range(self):
        doc, binary = read_glb()
        for mesh in doc['meshes']:
            for part in mesh['primitives']:
                index = part['attributes'].get('COLOR_0')
                if index is not None:
                    for pixel in floats(doc, binary, index):
                        self.assertTrue(all(0 <= channel <= 1 for channel in pixel))

    def test_face_opens_in_its_neutral_pose_and_exposes_expression_controls(self):
        doc, _ = read_glb()
        controls = set()
        for mesh in doc['meshes']:
            controls.update(mesh.get('extras', {}).get('targetNames', []))
            self.assertTrue(all(weight == 0 for weight in mesh.get('weights', [])))
        self.assertTrue({'Blink', 'Smile', 'Surprise', 'Concern'} <= controls)

    def test_six_clips_contain_real_motion_and_the_loops_close(self):
        doc, binary = read_glb()
        clips = {clip['name']: clip for clip in doc.get('animations', [])}
        self.assertEqual(set(clips), {'Idle', 'Working', 'Waiting', 'NeedsInput', 'Celebrate', 'Error'})
        for name, clip in clips.items():
            moving = False
            for sampler in clip['samplers']:
                times = floats(doc, binary, sampler['input'])
                self.assertEqual(times[0][0], 0)
                self.assertGreater(times[-1][0], 1)
                values = floats(doc, binary, sampler['output'])
                moving |= any(row != values[0] for row in values[1:])
                if name in {'Idle', 'Working', 'Waiting', 'NeedsInput'}:
                    for a, b in zip(values[0], values[-1]):
                        self.assertAlmostEqual(a, b, places=5)
            self.assertTrue(moving, name + ' contains no motion')

    def test_preset_exports_work_without_custom_visibility_code(self):
        for preset, expected in [('mint', {'Accessory_Satchel'}),
                                 ('lavender', {'Accessory_Satchel', 'Accessory_Glasses'}),
                                 ('peach', {'Accessory_Satchel', 'Accessory_Scarf'})]:
            doc, _ = read_glb(ASSET.with_name('pixie-' + preset + '.glb'))
            accessories = {node['name'] for node in doc['nodes'] if node.get('name', '').startswith('Accessory_')}
            self.assertEqual(accessories, expected)
            self.assertEqual(len(doc['animations']), 6)


if __name__ == '__main__':
    unittest.main()
