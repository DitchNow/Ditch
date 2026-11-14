import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../data/prompt_attachment.dart';
import '../design_system/ditch_theme.dart';

class PromptAttachments extends StatelessWidget {
  const PromptAttachments({
    required this.attachments,
    required this.onRemove,
    super.key,
  });
  final List<PromptAttachment> attachments;
  final ValueChanged<PromptAttachment>? onRemove;

  Future<void> _preview(
    BuildContext context,
    PromptAttachment attachment,
  ) async {
    if (attachment.previewKind == AttachmentPreviewKind.image) {
      await showDialog<void>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(attachment.filename),
          content: SizedBox(
            width: 640,
            height: MediaQuery.sizeOf(context).height * 0.6,
            child: InteractiveViewer(
              minScale: 0.5,
              maxScale: 5,
              child: Center(
                child: Image.file(
                  File(attachment.path),
                  fit: BoxFit.contain,
                  errorBuilder: (_, _, _) =>
                      const Text('This image could not be previewed.'),
                ),
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Close'),
            ),
          ],
        ),
      );
      return;
    }
    try {
      final opened = await const MethodChannel(
        'the_ditch/project_picker',
      ).invokeMethod<bool>('openFile', attachment.path);
      if (opened != true) {
        throw StateError('No application could open this file');
      }
    } on Object catch (_) {
      if (context.mounted) {
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          SnackBar(content: Text('Could not open ${attachment.filename}.')),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context) => SizedBox(
    height: 56,
    child: ListView.separated(
      scrollDirection: Axis.horizontal,
      itemCount: attachments.length,
      separatorBuilder: (_, _) => const SizedBox(width: 8),
      itemBuilder: (context, index) {
        final attachment = attachments[index];
        const fallback = Icon(Icons.insert_drive_file_outlined, size: 24);
        return Tooltip(
          message: attachment.path,
          child: Semantics(
            label: 'Included file ${attachment.filename}',
            child: Container(
              width: 190,
              padding: const EdgeInsets.all(4),
              decoration: BoxDecoration(
                color: context.ditch.surface,
                borderRadius: BorderRadius.circular(context.ditch.radiusMedium),
                border: Border.all(color: context.ditch.separator),
              ),
              child: Row(
                children: [
                  Expanded(
                    child: InkWell(
                      onTap: () => _preview(context, attachment),
                      child: Row(
                        children: [
                          SizedBox(
                            width: 40,
                            height: 40,
                            child:
                                attachment.previewKind ==
                                    AttachmentPreviewKind.image
                                ? Image.file(
                                    File(attachment.path),
                                    fit: BoxFit.cover,
                                    cacheWidth: 128,
                                    cacheHeight: 128,
                                    errorBuilder: (_, _, _) => fallback,
                                  )
                                : fallback,
                          ),
                          const SizedBox(width: 6),
                          Expanded(
                            child: Text(
                              attachment.filename,
                              maxLines: 2,
                              overflow: TextOverflow.ellipsis,
                              style: Theme.of(context).textTheme.labelSmall,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                  IconButton(
                    tooltip: 'Remove ${attachment.filename}',
                    style: IconButton.styleFrom(
                      backgroundColor: context.ditch.surface,
                      minimumSize: const Size(24, 24),
                      maximumSize: const Size(24, 24),
                      tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      padding: EdgeInsets.zero,
                    ),
                    constraints: const BoxConstraints.tightFor(
                      width: 24,
                      height: 24,
                    ),
                    padding: EdgeInsets.zero,
                    iconSize: 16,
                    onPressed: onRemove == null
                        ? null
                        : () => onRemove!(attachment),
                    icon: const Icon(Icons.close),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    ),
  );
}
