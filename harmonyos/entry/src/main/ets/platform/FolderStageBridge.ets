import { shellString } from './ShellLocalization';
import { BusinessError } from '@kit.BasicServicesKit';
import { FolderDestinationBridge, isFolderDestinationHandle, isFolderOutputIntent } from './FolderDestinationBridge';
import { FolderPublicationBridge } from './FolderPublicationBridge';
import { FolderStageCompletion, FolderStageRequest } from './FolderOutputCoordinator';
import { validateFolderPublicationIdentity } from './FolderPublicationTransaction';

/** Resolve the exact preauthorized target before allocating an otherwise empty worker-owned output tree. */
export class FolderStageBridge {
  private destinations: FolderDestinationBridge;
  private publications: FolderPublicationBridge;

  constructor(destinations: FolderDestinationBridge, publications: FolderPublicationBridge) {
    this.destinations = destinations;
    this.publications = publications;
  }

  async allocate(request: FolderStageRequest): Promise<FolderStageCompletion> {
    const completion: FolderStageCompletion = { kind: 'folderStageComplete', id: request.id, target: request.target,
      result: 'error', destinationHandle: request.destinationHandle, ownerRoot: '', stageRoot: '', journalPath: '', error: '' };
    try {
      validateFolderPublicationIdentity(request.id, request.target);
      if (request.kind !== 'folderStageAllocate' || !isFolderOutputIntent(request.intent) ||
        !isFolderDestinationHandle(request.destinationHandle)) { throw new Error(shellString("shell_invalid_output_task_or_folder_selection_identifier", "Invalid output task or folder selection identifier")); }
      await this.destinations.resolve(request.destinationHandle, request.target);
      const stage = await this.publications.createStage(request.id, request.target);
      completion.ownerRoot = stage.ownerRoot;
      completion.stageRoot = stage.stageRoot;
      completion.journalPath = stage.journalPath;
      completion.result = 'success';
    } catch (error) {
      const code = (error as BusinessError).code;
      completion.error = typeof code === 'number' ? shellString("shell_could_not_prepare_the_output_folder_error_detail_select", "Could not prepare the output folder (error {0}). Select it again and retry", code) :
        error instanceof SyntaxError ? shellString("shell_invalid_folder_selection_record_select_the_folder_again_and", "Invalid folder selection record. Select the folder again and retry") :
          (error as Error).message || shellString("shell_could_not_prepare_the_output_folder_select_it_again", "Could not prepare the output folder. Select it again and retry");
    }
    return completion;
  }
}
