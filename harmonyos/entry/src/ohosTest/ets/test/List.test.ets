import photoCraftDeviceTests from './PhotoCraftDevice.test';
import photoCraftPhaseTests from './PhotoCraftPhases.test';

export default function testsuite(): void {
  photoCraftDeviceTests();
  photoCraftPhaseTests();
}
