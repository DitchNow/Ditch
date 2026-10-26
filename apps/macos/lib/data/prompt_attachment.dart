import 'dart:convert';
import 'dart:io';

import 'package:path/path.dart' as p;

enum AttachmentPreviewKind { image, file }

/// A local reference, never an upload or an embedded runtime payload.
class PromptAttachment {
  PromptAttachment(String path) : path = p.normalize(p.absolute(path));

  final String path;
  String get filename => p.basename(path);
  AttachmentPreviewKind get previewKind =>
      const {
        '.jpg',
        '.jpeg',
        '.png',
        '.webp',
      }.contains(p.extension(path).toLowerCase())
      ? AttachmentPreviewKind.image
      : AttachmentPreviewKind.file;

  Future<bool> get isReadable async {
    try {
      if (await FileSystemEntity.type(path) != FileSystemEntityType.file) {
        return false;
      }
      final handle = await File(path).open();
      await handle.close();
      return true;
    } on FileSystemException {
      return false;
    }
  }
}

String serializeAttachedPrompt(
  String text,
  List<PromptAttachment> attachments,
) {
  final prompt = text.trim();
  if (attachments.isEmpty) return prompt;
  final references =
      'Included local files (absolute paths, JSON array):\n'
      '${jsonEncode(attachments.map((attachment) => attachment.path).toList())}';
  return prompt.isEmpty ? references : '$prompt\n\n$references';
}
